pub mod audio;
pub mod commands;
pub mod db;
pub mod flow;
pub mod library;
pub mod media_controls;
pub mod player;
pub mod protocol;
pub mod settings;
pub mod shell;
pub mod startup;
pub mod thumbs;
pub mod tools;
pub mod window;

use std::sync::{Arc, Mutex};

use anyhow::Context;
use tauri::Manager;

use audio::CpalOutputFactory;
use library::covers::CoverStore;
use library::scan::lock;
use library::{query, tauri_emitter, watch, Library, LibraryOptions};
use media_controls::MediaBridge;
use player::{
    default_opener, fan_out, tauri_sink, with_dropped, DroppedFiles, Player, PlayerOptions,
    Resolver,
};
use settings::SettingsStore;
use startup::{
    fail_with, open_or_quarantine, AppDirs, CannotCreateDataDir, DataDir, OpenOutcome,
    StartupNotice,
};

/// Shared backend state, managed by Tauri.
pub struct AppState {
    pub library: Arc<Library>,
    pub player: Player,
    /// Files dropped onto the window, playable without indexing.
    pub dropped: Arc<DroppedFiles>,
    pub settings: Arc<SettingsStore>,
    /// Limits and coalesces cover resizes.
    pub thumbs: Arc<thumbs::ResizeGate>,
    /// Online cover lookup for albums without a cover.
    pub lookup: library::lookup::LookupWorker,
}

/// Resolves queue entries against the library database.
fn library_resolver(library: &Library) -> Resolver {
    let db = Arc::clone(&library.db);
    Arc::new(move |id: &str| {
        let conn = lock(&db).ok()?;
        query::track(&conn, id).ok().flatten()
    })
}

/// The native handle media controls attach to: the main window's HWND on Windows, nothing
/// elsewhere.
fn main_window_handle(app: &tauri::App) -> Option<usize> {
    #[cfg(target_os = "windows")]
    {
        match app.get_webview_window("main").map(|window| window.hwnd()) {
            Some(Ok(hwnd)) => Some(hwnd.0 as usize),
            _ => None,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        None
    }
}

/// Where Prism keeps its data, config and cache. `PRISM_DATA_DIR` relocates all three, in
/// debug builds only, so a dev run never touches the real profile.
fn app_dirs(app: &tauri::App) -> anyhow::Result<AppDirs> {
    #[cfg(debug_assertions)]
    if let Some(dirs) = startup::dirs_under_override(std::env::var_os("PRISM_DATA_DIR").as_deref())
    {
        return Ok(dirs);
    }
    let path = app.path();
    Ok(AppDirs {
        data: path.app_data_dir().context("resolving the app data dir")?,
        config: path
            .app_config_dir()
            .context("resolving the app config dir")?,
        cache: path
            .app_cache_dir()
            .context("resolving the app cache dir")?,
    })
}

fn setup(app: &mut tauri::App) -> anyhow::Result<()> {
    // First, so the glass is in place before the page paints.
    window::setup(app);
    let dirs = app_dirs(app)?;
    let data_dir = dirs.data;
    std::fs::create_dir_all(&data_dir).map_err(|source| CannotCreateDataDir {
        path: data_dir.clone(),
        source,
    })?;
    let cover_cache = dirs.cache.join("covers");
    let cover_dir = cover_cache.join("original");

    let config_dir = dirs.config;
    let settings = Arc::new(SettingsStore::load(&config_dir.join("settings.json")));
    let initial = settings.get();

    let (db, notice) = match open_or_quarantine(&data_dir.join("library.db"))
        .map_err(|err| err.context(DataDir(data_dir.clone())))?
    {
        OpenOutcome::Opened(db) => (db, None),
        OpenOutcome::Quarantined { db, bad } => (
            db,
            bad.file_name()
                .map(|name| name.to_string_lossy().into_owned()),
        ),
    };
    app.manage(StartupNotice(Mutex::new(notice)));
    let db = Arc::new(db);
    let library = Arc::new(Library::new(
        db,
        Arc::new(CoverStore::new(cover_dir)),
        LibraryOptions {
            reread_unparsed: tools::ffprobe_path().is_some(),
        },
    ));
    let folders = library.folders()?;
    let handle = app.handle().clone();
    let media = MediaBridge::new();
    let dropped = Arc::new(DroppedFiles::new(Some(Arc::clone(&library.covers))));
    let player = Player::spawn_with_options(
        Arc::new(CpalOutputFactory),
        with_dropped(Arc::clone(&dropped), library_resolver(&library)),
        fan_out(vec![
            tauri_sink(handle.clone()),
            media.sink(),
            library::history::spawn_recorder(Arc::clone(&library.db)),
        ]),
        default_opener(),
        PlayerOptions {
            volume: initial.volume,
            device: initial.output_device,
            gapless: initial.gapless,
            replay_gain: initial.replay_gain,
        },
    );
    let lookup = library::lookup::LookupWorker::spawn(
        Arc::clone(&library.db),
        Arc::clone(&library.covers),
        Arc::new(library::lookup::MusicBrainz::new()?),
        Arc::new(std::sync::atomic::AtomicBool::new(initial.cover_lookup)),
        {
            let handle = handle.clone();
            Arc::new(move || {
                if let Err(err) = tauri::Emitter::emit(&handle, library::EVENT_CHANGED, ()) {
                    eprintln!("emitting the cover change failed: {err}");
                }
            })
        },
    );
    app.manage(AppState {
        lookup,
        library: Arc::clone(&library),
        player,
        dropped,
        settings,
        thumbs: Arc::new(thumbs::ResizeGate::new(thumbs::ResizeGate::default_limit())),
    });
    media.start(handle.clone(), main_window_handle(app), cover_cache);

    watch::start(handle.clone(), folders);

    // Catch up on changes made while Prism was closed; unchanged files are skipped quickly.
    std::thread::spawn(move || {
        let emit = tauri_emitter(handle.clone());
        if let Err(err) = library.rescan_all(&emit) {
            eprintln!("startup rescan failed: {err:#}");
        }
        if let Some(state) = handle.try_state::<AppState>() {
            state.lookup.kick();
        }
    });
    Ok(())
}

pub fn run() {
    // The web view keeps its profile (cookies, cache) in its own folder; relocate it too.
    #[cfg(all(debug_assertions, target_os = "windows"))]
    if let Some(dirs) = startup::dirs_under_override(std::env::var_os("PRISM_DATA_DIR").as_deref())
    {
        std::env::set_var(
            "WEBVIEW2_USER_DATA_FOLDER",
            dirs.data.with_file_name("webview"),
        );
    }
    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("prism", protocol::register)
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| match setup(app) {
            Ok(()) => Ok(()),
            Err(err) => fail_with(&err),
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                window::on_main_event(window, event);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::player_play,
            commands::player_toggle,
            commands::player_next,
            commands::player_prev,
            commands::player_seek,
            commands::player_set_volume,
            commands::player_state,
            commands::player_play_files,
            commands::player_append,
            commands::player_jump,
            commands::player_queue_range,
            commands::library_add_folder,
            commands::library_remove_folder,
            commands::library_folders,
            commands::library_rescan,
            commands::library_albums,
            commands::library_album_tracks,
            commands::library_artists,
            commands::library_artist_albums,
            commands::library_track_count,
            commands::library_tracks_range,
            commands::library_search,
            commands::favorites_set,
            commands::favorites_ids,
            commands::favorites_tracks,
            commands::favorites_albums,
            commands::favorites_artists,
            commands::playlists_list,
            commands::playlist_create,
            commands::playlist_rename,
            commands::playlist_delete,
            commands::playlist_entries,
            commands::playlist_add,
            commands::playlist_remove,
            commands::playlist_move,
            commands::settings_get,
            commands::settings_set,
            commands::audio_devices,
            window::window_vibrancy,
            startup::startup_notice,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|err| fail_with(&anyhow::Error::new(err)))
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    state.settings.flush();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_builds() {
        assert_eq!(2 + 2, 4);
    }
}
