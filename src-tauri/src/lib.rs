pub mod audio;
pub mod commands;
pub mod db;
pub mod flow;
pub mod library;
pub mod media_controls;
pub mod navidrome;
pub mod player;
pub mod protocol;
pub mod secret;
pub mod settings;
pub mod shell;
pub mod startup;
pub mod thumbs;
pub mod tools;
pub mod window;

use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Context;
use tauri::Manager;

use audio::CpalOutputFactory;
use flow::FlowService;
use library::covers::CoverStore;
use library::scan::lock;
use library::{query, tauri_emitter, watch, Library, LibraryOptions};
use media_controls::MediaBridge;
use navidrome::keychain::{CredentialStore, KeyringStore};
use navidrome::stream::navidrome_opener;
use navidrome::{NavidromeEmit, NavidromeEvent, NavidromeService};
use player::{
    default_opener, fan_out, tauri_sink, with_dropped, DroppedFiles, EventSink, Player,
    PlayerOptions, Resolver,
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
    /// Starts Flows and keeps them going.
    pub flow: Arc<FlowService>,
    /// Files dropped onto the window, playable without indexing.
    pub dropped: Arc<DroppedFiles>,
    pub settings: Arc<SettingsStore>,
    /// Limits and coalesces cover resizes.
    pub thumbs: Arc<thumbs::ResizeGate>,
    /// Online cover lookup for albums without a cover.
    pub lookup: library::lookup::LookupWorker,
    /// The Navidrome account, its status and the background sync.
    pub navidrome: Arc<NavidromeService>,
}

/// Sends the Navidrome service's events to every window.
fn navidrome_emit(handle: tauri::AppHandle) -> NavidromeEmit {
    use tauri::Emitter;
    Arc::new(move |event| {
        let (name, sent) = match &event {
            NavidromeEvent::Status(status) => (
                navidrome::EVENT_STATUS,
                handle.emit(navidrome::EVENT_STATUS, status),
            ),
            NavidromeEvent::LibraryChanged => (
                library::EVENT_CHANGED,
                handle.emit(library::EVENT_CHANGED, ()),
            ),
            NavidromeEvent::FavoritesChanged => (
                library::favorites::EVENT_CHANGED,
                handle.emit(library::favorites::EVENT_CHANGED, ()),
            ),
            NavidromeEvent::PlaylistsChanged => (
                library::playlists::EVENT_CHANGED,
                handle.emit(library::playlists::EVENT_CHANGED, ()),
            ),
            NavidromeEvent::SettingsChanged(settings) => (
                settings::EVENT_CHANGED,
                handle.emit(settings::EVENT_CHANGED, settings),
            ),
        };
        if let Err(err) = sent {
            eprintln!("emitting {name} failed: {err}");
        }
    })
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
    // First, so the transparent background and the window shape are in place before the
    // page paints.
    window::setup(app);
    let dirs = app_dirs(app)?;
    let data_dir = dirs.data;
    std::fs::create_dir_all(&data_dir).map_err(|source| CannotCreateDataDir {
        path: data_dir.clone(),
        source,
    })?;
    let cover_cache = dirs.cache.join("covers");
    let cover_dir = cover_cache.join("original");
    // Stream buffers left behind by a crash.
    audio::http::cleanup_spill_dir(&audio::http::spill_dir(&dirs.cache));

    let config_dir = dirs.config;
    let settings = Arc::new(SettingsStore::load(&config_dir.join("settings.json")));
    // The system's own sign-in entry wins over the saved value (only read here).
    let saved_start = settings.get().start_with_system;
    if let Some(on) = shell::shortcut::start_with_system_on_os(app.handle(), saved_start) {
        if let Err(err) = settings.set(serde_json::json!({ "startWithSystem": on })) {
            eprintln!("following the start with the system failed: {err:#}");
        }
    }
    let initial = settings.get();
    app.manage(shell::ShellState::default());
    app.manage(shell::shortcut::CurrentShortcut::default());

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
    // The Flow service needs the player's handle and the player needs the service's sink:
    // the sink is plugged in once both exist.
    let flow_sink: Arc<OnceLock<EventSink>> = Arc::default();
    let to_flow = Arc::clone(&flow_sink);
    // Keychain entries are keyed by the app identifier: a dev run never sees the real password.
    // Created before the player, whose opener streams Navidrome tracks through it.
    let navidrome = NavidromeService::spawn(
        Arc::clone(&library.db),
        Arc::clone(&settings),
        Arc::new(KeyringStore::new(&app.config().identifier)) as Arc<dyn CredentialStore>,
        navidrome_emit(handle.clone()),
    );
    let player = Player::spawn_with_options(
        Arc::new(CpalOutputFactory),
        with_dropped(Arc::clone(&dropped), library_resolver(&library)),
        fan_out(vec![
            tauri_sink(handle.clone()),
            media.sink(),
            shell::player_sink(handle.clone()),
            library::history::spawn_recorder(Arc::clone(&library.db)),
            Arc::new(move |event| {
                if let Some(sink) = to_flow.get() {
                    sink(event);
                }
            }),
        ]),
        navidrome_opener(
            Arc::clone(&navidrome),
            Arc::clone(&settings),
            audio::http::spill_dir(&dirs.cache),
            default_opener(),
        ),
        PlayerOptions {
            volume: initial.volume,
            device: initial.output_device,
            gapless: initial.gapless,
            replay_gain: initial.replay_gain,
            crossfade: initial.crossfade,
            crossfade_ms: initial.crossfade_ms,
            repeat: initial.repeat,
            shuffle: initial.shuffle,
        },
    );
    let lookup = library::lookup::LookupWorker::spawn(
        Arc::clone(&library.db),
        Arc::clone(&library.covers),
        match library::lookup::MusicBrainz::new() {
            Ok(fetcher) => Arc::new(fetcher) as Arc<dyn library::lookup::CoverFetcher>,
            Err(err) => {
                eprintln!("cover lookup is off: {err:#}");
                Arc::new(library::lookup::Unavailable)
            }
        },
        Arc::new(std::sync::atomic::AtomicBool::new(initial.cover_lookup)),
        {
            let handle = handle.clone();
            Arc::new(move || {
                // On the lookup thread: re-read the playing track so a new cover shows at once.
                if let Some(state) = handle.try_state::<AppState>() {
                    if let Some(shown) = state.player.state().track {
                        let fresh = lock(&state.library.db)
                            .ok()
                            .and_then(|conn| query::track(&conn, &shown.id).ok().flatten());
                        if let Some(fresh) = fresh {
                            let _ = state.player.refresh_track(fresh);
                        }
                    }
                }
                if let Err(err) = tauri::Emitter::emit(&handle, library::EVENT_CHANGED, ()) {
                    eprintln!("emitting the cover change failed: {err}");
                }
            })
        },
    );
    let (flow, sink) = FlowService::spawn(Arc::clone(&library.db), player.handle());
    let _ = flow_sink.set(sink);
    app.manage(AppState {
        navidrome,
        lookup,
        library: Arc::clone(&library),
        player,
        flow,
        dropped,
        settings,
        thumbs: Arc::new(thumbs::ResizeGate::new(thumbs::ResizeGate::default_limit())),
    });
    // After the app state: the mini page asks for it as soon as it loads. Hidden until the
    // user asks for it.
    if let Err(err) = shell::mini::create(app.handle(), initial.mini_on_top) {
        eprintln!("creating the mini player failed: {err}");
    }
    if let Err(err) = shell::shortcut::apply(&handle, &initial.mini_shortcut) {
        eprintln!("registering the mini player shortcut failed: {err}");
    }
    media.start(handle.clone(), main_window_handle(app), cover_cache);
    // The tray: closing the main window leaves Prism there (and the taskbar button goes).
    if let Err(err) = shell::tray::create(&handle) {
        eprintln!("creating the tray icon failed: {err}");
    }
    #[cfg(target_os = "windows")]
    if let Some(window) = app.get_webview_window(shell::MAIN_LABEL) {
        shell::taskbar_win::attach(&handle, &window);
    }

    watch::start(handle.clone(), folders);
    let args: Vec<String> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    shell::start_main(&handle, shell::shortcut::is_autostart(&args));
    shell::open_with::play_launch_args(&handle);

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
        // First, so a second start hands its files over before anything else initializes.
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            shell::open_with::on_second_instance(app, &args, &cwd);
        }))
        .register_asynchronous_uri_scheme_protocol("prism", protocol::register)
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(shell::shortcut::on_shortcut)
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![shell::shortcut::AUTOSTART_ARG]),
        ))
        .setup(|app| match setup(app) {
            Ok(()) => Ok(()),
            Err(err) => fail_with(&err),
        })
        .on_window_event(|window, event| {
            if window.label() == shell::MAIN_LABEL {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    // Prism keeps playing: the main window steps aside (`shell::close_main`).
                    api.prevent_close();
                    shell::close_main(window);
                    return;
                }
                window::on_main_event(window, event);
            } else if window.label() == shell::mini::MINI_LABEL {
                shell::mini::on_event(window, event);
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
            commands::player_move,
            commands::player_remove,
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
            commands::library_genres,
            commands::flow_start,
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
            commands::navidrome_status,
            commands::navidrome_test,
            commands::navidrome_connect,
            commands::navidrome_disconnect,
            commands::navidrome_sync,
            commands::mini_toggle,
            commands::mini_layout,
            commands::mini_menu,
            commands::main_show,
            commands::tray_set_labels,
            window::window_vibrancy,
            startup::startup_notice,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|err| fail_with(&anyhow::Error::new(err)))
        .run(|app, event| match event {
            // Windows closed on the way out bring no mini player up (`shell::close_main`).
            tauri::RunEvent::ExitRequested { .. } => {
                if let Some(state) = app.try_state::<shell::ShellState>() {
                    state.set_quitting();
                }
            }
            tauri::RunEvent::Exit => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.settings.flush();
                    state.navidrome.shutdown();
                }
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Opened { urls } => {
                shell::open_with::play_paths(app, shell::open_with::paths_from_urls(&urls));
            }
            // A click on the Dock icon brings the main window back.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => {
                if let Err(err) = shell::main_show(app) {
                    eprintln!("opening Prism failed: {err}");
                }
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_builds() {
        assert_eq!(2 + 2, 4);
    }
}
