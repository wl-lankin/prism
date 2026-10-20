//! Tauri commands. Thin wrappers: the player and library services hold the logic. Every
//! command returns `Result<T, String>` with a message fit for the UI.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::audio::list_devices;
use crate::flow::{FlowOutcome, FlowVariant};
use crate::library::favorites::{self, FavKind, FavoriteIds};
use crate::library::model::{Album, Artist, Folder, Track};
use crate::library::playlists::{self, Playlist, PlaylistEntry};
use crate::library::query::{self, SearchResult};
use crate::library::scan::{lock, ScanReport};
use crate::library::watch::{FolderWatcher, WatchState};
use crate::library::{normalize, tauri_emitter, Library};
use crate::player::{PlayerState, QueueItem};
use crate::settings::{Change, Settings};
use crate::shell;
use crate::shell::mini::{self, Anchor, MiniLayout};
use crate::AppState;

/// Upper bound for `library_search` and `library_tracks_range` page sizes.
const MAX_PAGE: u32 = 1_000;

fn message(err: anyhow::Error) -> String {
    format!("{err:#}")
}

fn library(app: &AppHandle) -> Arc<Library> {
    Arc::clone(&app.state::<AppState>().library)
}

/// Runs blocking work (scans, queries) off the async runtime's worker threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|err| format!("the library task failed: {err}"))?
        .map_err(message)
}

/// Runs a read query against the library database.
async fn read<T: Send + 'static>(
    app: &AppHandle,
    run: impl FnOnce(&Connection) -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    let library = library(app);
    blocking(move || run(&*lock(&library.db)?)).await
}

fn with_watcher(app: &AppHandle, act: impl FnOnce(&mut FolderWatcher) -> anyhow::Result<()>) {
    let Some(state) = app.try_state::<WatchState>() else {
        return;
    };
    let Ok(mut watcher) = state.0.lock() else {
        return;
    };
    if let Some(watcher) = watcher.as_mut() {
        if let Err(err) = act(watcher) {
            eprintln!("{err:#}");
        }
    }
}

fn folder_path(path: &str) -> Result<PathBuf, String> {
    normalize(Path::new(path)).map_err(message)
}

// Player

#[tauri::command]
pub fn player_play(
    state: State<'_, AppState>,
    track_ids: Vec<String>,
    start_index: u32,
) -> Result<(), String> {
    state.player.play(track_ids, start_index as usize)
}

#[tauri::command]
pub fn player_toggle(state: State<'_, AppState>) -> Result<(), String> {
    state.player.toggle()
}

#[tauri::command]
pub fn player_next(state: State<'_, AppState>) -> Result<(), String> {
    state.player.next()
}

#[tauri::command]
pub fn player_prev(state: State<'_, AppState>) -> Result<(), String> {
    state.player.prev()
}

#[tauri::command]
pub fn player_seek(state: State<'_, AppState>, ms: u64) -> Result<(), String> {
    state.player.seek(ms)
}

#[tauri::command]
pub fn player_set_volume(state: State<'_, AppState>, v: f32) -> Result<(), String> {
    state.settings.remember_volume(v);
    state.player.set_volume(v)
}

/// Plays dropped files without indexing them: tags are read on the fly, folders and non-audio
/// files and network paths (UNC) are skipped. Returns how many files are queued; with none,
/// the player is untouched. The dropped tracks are kept only when playback was accepted.
#[tauri::command]
pub async fn player_play_files(app: AppHandle, paths: Vec<String>) -> Result<u32, String> {
    let dropped = Arc::clone(&app.state::<AppState>().dropped);
    let reader = Arc::clone(&dropped);
    let tracks = blocking(move || Ok(reader.read(&paths))).await?;
    let state = app.state::<AppState>();
    dropped.commit_and_play(tracks, |ids| state.player.play(ids, 0))
}

#[tauri::command]
pub fn player_state(state: State<'_, AppState>) -> Result<PlayerState, String> {
    Ok(state.player.state())
}

/// Adds tracks at the end of the queue; what plays now keeps playing.
#[tauri::command]
pub fn player_append(state: State<'_, AppState>, track_ids: Vec<String>) -> Result<(), String> {
    state.player.append(track_ids)
}

/// Plays the queue entry at `index`; an index out of range is ignored.
#[tauri::command]
pub fn player_jump(state: State<'_, AppState>, index: u32) -> Result<(), String> {
    state.player.jump(index as usize)
}

/// A page of the queue. Resolving takes the library DB lock, so it runs off the async
/// runtime's worker threads.
#[tauri::command]
pub async fn player_queue_range(
    app: AppHandle,
    offset: u32,
    limit: u32,
) -> Result<Vec<QueueItem>, String> {
    let limit = limit.min(MAX_PAGE);
    blocking(move || Ok(app.state::<AppState>().player.queue_range(offset, limit))).await
}

// Library

/// Adds a folder, indexes it and starts watching it. Folders inside or around an existing
/// library folder are refused.
#[tauri::command]
pub async fn library_add_folder(app: AppHandle, path: String) -> Result<ScanReport, String> {
    let folder = folder_path(&path)?;
    let library = library(&app);
    // Watcher calls touch the file system too, so they run in the blocking task as well.
    blocking(move || {
        let report = library.add_folder(&folder, &tauri_emitter(app.clone()))?;
        with_watcher(&app, |watcher| watcher.watch(&folder));
        Ok(report)
    })
    .await
}

/// Stops watching a folder and forgets it and its tracks.
#[tauri::command]
pub async fn library_remove_folder(app: AppHandle, path: String) -> Result<(), String> {
    let folder = folder_path(&path)?;
    let library = library(&app);
    blocking(move || {
        library.remove_folder(&folder, &tauri_emitter(app.clone()))?;
        with_watcher(&app, |watcher| watcher.unwatch(&folder));
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn library_folders(app: AppHandle) -> Result<Vec<Folder>, String> {
    read(&app, query::folders).await
}

/// Rescans every library folder; returns when done.
#[tauri::command]
pub async fn library_rescan(app: AppHandle) -> Result<(), String> {
    let library = library(&app);
    let emit_app = app.clone();
    blocking(move || {
        library.rescan_all(&tauri_emitter(emit_app))?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn library_albums(app: AppHandle) -> Result<Vec<Album>, String> {
    read(&app, query::albums).await
}

#[tauri::command]
pub async fn library_album_tracks(app: AppHandle, album_id: String) -> Result<Vec<Track>, String> {
    read(&app, move |conn| query::album_tracks(conn, &album_id)).await
}

#[tauri::command]
pub async fn library_artists(app: AppHandle) -> Result<Vec<Artist>, String> {
    read(&app, query::artists).await
}

#[tauri::command]
pub async fn library_artist_albums(app: AppHandle, name: String) -> Result<Vec<Album>, String> {
    read(&app, move |conn| query::artist_albums(conn, &name)).await
}

#[tauri::command]
pub async fn library_track_count(app: AppHandle) -> Result<u32, String> {
    read(&app, query::track_count).await
}

#[tauri::command]
pub async fn library_tracks_range(
    app: AppHandle,
    offset: u32,
    limit: u32,
) -> Result<Vec<Track>, String> {
    let limit = limit.min(MAX_PAGE);
    read(&app, move |conn| query::tracks_range(conn, offset, limit)).await
}

#[tauri::command]
pub async fn library_search(app: AppHandle, q: String, limit: u32) -> Result<SearchResult, String> {
    let limit = limit.min(MAX_PAGE);
    read(&app, move |conn| query::search(conn, &q, limit)).await
}

/// Every genre once, blank ones left out, sorted case-insensitively.
#[tauri::command]
pub async fn library_genres(app: AppHandle) -> Result<Vec<String>, String> {
    read(&app, query::genres).await
}

// Flow

/// Plays a new Flow; favorites without any fall back to the whole library.
#[tauri::command]
pub async fn flow_start(app: AppHandle, variant: FlowVariant) -> Result<FlowOutcome, String> {
    let flow = Arc::clone(&app.state::<AppState>().flow);
    blocking(move || flow.start(variant)).await
}

// Favorites

/// Sets a favorite and returns the new state; emits `favorites://changed` when it changed.
#[tauri::command]
pub async fn favorites_set(
    app: AppHandle,
    kind: FavKind,
    id: String,
    on: bool,
) -> Result<bool, String> {
    let changed = read(&app, move |conn| {
        favorites::set_changed(conn, kind, &id, on)
    })
    .await?;
    if changed {
        if let Err(err) = app.emit(favorites::EVENT_CHANGED, ()) {
            eprintln!("emitting {} failed: {err}", favorites::EVENT_CHANGED);
        }
    }
    Ok(on)
}

#[tauri::command]
pub async fn favorites_ids(app: AppHandle) -> Result<FavoriteIds, String> {
    read(&app, favorites::ids).await
}

#[tauri::command]
pub async fn favorites_tracks(app: AppHandle) -> Result<Vec<Track>, String> {
    read(&app, favorites::tracks).await
}

#[tauri::command]
pub async fn favorites_albums(app: AppHandle) -> Result<Vec<Album>, String> {
    read(&app, favorites::albums).await
}

#[tauri::command]
pub async fn favorites_artists(app: AppHandle) -> Result<Vec<Artist>, String> {
    read(&app, favorites::artists).await
}

// Playlists

fn emit_playlists_changed(app: &AppHandle) {
    if let Err(err) = app.emit(playlists::EVENT_CHANGED, ()) {
        eprintln!("emitting {} failed: {err}", playlists::EVENT_CHANGED);
    }
}

#[tauri::command]
pub async fn playlists_list(app: AppHandle) -> Result<Vec<Playlist>, String> {
    read(&app, playlists::playlists_list).await
}

#[tauri::command]
pub async fn playlist_create(app: AppHandle, name: String) -> Result<Playlist, String> {
    let playlist = read(&app, move |conn| playlists::playlist_create(conn, &name)).await?;
    emit_playlists_changed(&app);
    Ok(playlist)
}

#[tauri::command]
pub async fn playlist_rename(app: AppHandle, id: i64, name: String) -> Result<(), String> {
    read(&app, move |conn| {
        playlists::playlist_rename(conn, id, &name)
    })
    .await?;
    emit_playlists_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn playlist_delete(app: AppHandle, id: i64) -> Result<(), String> {
    read(&app, move |conn| playlists::playlist_delete(conn, id)).await?;
    emit_playlists_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn playlist_entries(app: AppHandle, id: i64) -> Result<Vec<PlaylistEntry>, String> {
    read(&app, move |conn| playlists::playlist_entries(conn, id)).await
}

#[tauri::command]
pub async fn playlist_add(app: AppHandle, id: i64, track_ids: Vec<String>) -> Result<(), String> {
    read(&app, move |conn| {
        playlists::playlist_add(conn, id, &track_ids)
    })
    .await?;
    emit_playlists_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn playlist_remove(app: AppHandle, id: i64, positions: Vec<u32>) -> Result<(), String> {
    read(&app, move |conn| {
        playlists::playlist_remove(conn, id, &positions)
    })
    .await?;
    emit_playlists_changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn playlist_move(app: AppHandle, id: i64, from: u32, to: u32) -> Result<(), String> {
    read(&app, move |conn| {
        playlists::playlist_move(conn, id, from, to)
    })
    .await?;
    emit_playlists_changed(&app);
    Ok(())
}

// Settings

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.settings.get())
}

/// Merges `patch` into the settings, saves them and applies what changed to the player (or,
/// for the shell settings, to `shell::apply_setting`).
/// The player commands are sent while the settings are locked, so concurrent calls (a
/// debounced volume save during a device pick) reach the player once each and in order.
#[tauri::command]
pub async fn settings_set(app: AppHandle, patch: serde_json::Value) -> Result<Settings, String> {
    blocking(move || {
        let state = app.state::<AppState>();
        state.settings.update(patch, |changes| {
            for change in changes {
                let sent = match change {
                    Change::Volume(volume) => state.player.set_volume(*volume),
                    Change::OutputDevice(device) => state.player.set_device(device.clone()),
                    Change::Gapless(gapless) => state.player.set_gapless(*gapless),
                    Change::ReplayGain(mode) => state.player.set_replay_gain(*mode),
                    Change::Crossfade(mode, ms) => state.player.set_crossfade(*mode, *ms),
                    other => shell::apply_setting(&app, other),
                };
                if let Err(err) = sent {
                    eprintln!("applying a setting failed: {err}");
                }
            }
        })
    })
    .await
}

/// Names of the output devices, as `outputDevice` takes them.
#[tauri::command]
pub async fn audio_devices() -> Result<Vec<String>, String> {
    blocking(|| Ok(list_devices())).await
}

// Shell

/// Shows the mini player (bottom-right, without taking focus), or hides it.
#[tauri::command]
pub fn mini_toggle(app: AppHandle) -> Result<(), String> {
    mini::toggle(&app, &Anchor::WorkAreaBottomRight).map_err(|err| err.to_string())
}

/// Where the player sits in the mini window (the Flow menu's room is above or below it).
#[tauri::command]
pub fn mini_layout() -> MiniLayout {
    mini::layout()
}

/// The mini player's Flow menu opened or closed.
#[tauri::command]
pub fn mini_menu(app: AppHandle, open: bool) {
    mini::menu(&app, open);
}

/// Brings the main window up: shown, restored from the taskbar and focused.
#[tauri::command]
pub fn main_show(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("the main window is gone")?;
    let fail = |err: tauri::Error| err.to_string();
    window.show().map_err(fail)?;
    if window.is_minimized().unwrap_or(false) {
        window.unminimize().map_err(fail)?;
    }
    window.set_focus().map_err(fail)
}
