pub mod audio;
pub mod commands;
pub mod db;
pub mod library;
pub mod media_controls;
pub mod player;
pub mod protocol;
pub mod settings;
pub mod tools;
pub mod window;

use std::sync::Arc;

use anyhow::Context;
use tauri::Manager;

use audio::CpalOutputFactory;
use db::Db;
use library::covers::CoverStore;
use library::scan::lock;
use library::{query, tauri_emitter, watch, Library, LibraryOptions};
use media_controls::MediaBridge;
use player::{
    default_opener, fan_out, tauri_sink, with_dropped, DroppedFiles, Player, PlayerOptions,
    Resolver,
};
use settings::SettingsStore;

/// Shared backend state, managed by Tauri.
pub struct AppState {
    pub library: Arc<Library>,
    pub player: Player,
    /// Files dropped onto the window, playable without indexing.
    pub dropped: Arc<DroppedFiles>,
    pub settings: Arc<SettingsStore>,
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

fn setup(app: &mut tauri::App) -> anyhow::Result<()> {
    // First, so the glass is in place before the page paints.
    window::setup(app);
    let data_dir = app
        .path()
        .app_data_dir()
        .context("resolving the app data dir")?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let cover_cache = app
        .path()
        .app_cache_dir()
        .context("resolving the app cache dir")?
        .join("covers");
    let cover_dir = cover_cache.join("original");

    let config_dir = app
        .path()
        .app_config_dir()
        .context("resolving the app config dir")?;
    let settings = Arc::new(SettingsStore::load(&config_dir.join("settings.json")));
    let initial = settings.get();

    let db = Arc::new(Db::open(&data_dir.join("library.db"))?);
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
        fan_out(vec![tauri_sink(handle.clone()), media.sink()]),
        default_opener(),
        PlayerOptions {
            volume: initial.volume,
            device: initial.output_device,
            gapless: initial.gapless,
            replay_gain: initial.replay_gain,
        },
    );
    app.manage(AppState {
        library: Arc::clone(&library),
        player,
        dropped,
        settings,
    });
    media.start(handle.clone(), main_window_handle(app), cover_cache);

    watch::start(handle.clone(), folders);

    // Catch up on changes made while Prism was closed; unchanged files are skipped quickly.
    std::thread::spawn(move || {
        let emit = tauri_emitter(handle);
        if let Err(err) = library.rescan_all(&emit) {
            eprintln!("startup rescan failed: {err:#}");
        }
    });
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("prism", protocol::register)
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| setup(app).map_err(|err| err.into()))
        .invoke_handler(tauri::generate_handler![
            commands::player_play,
            commands::player_toggle,
            commands::player_next,
            commands::player_prev,
            commands::player_seek,
            commands::player_set_volume,
            commands::player_state,
            commands::player_play_files,
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
            commands::settings_get,
            commands::settings_set,
            commands::audio_devices,
            window::window_vibrancy,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Prism");
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_builds() {
        assert_eq!(2 + 2, 4);
    }
}
