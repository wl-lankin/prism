//! Tauri commands. Thin wrappers: the player and library services hold the logic. Every
//! command returns `Result<T, String>` with a message fit for the UI.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;
use tauri::{AppHandle, Manager, State};

use crate::library::model::{Album, Artist, Folder, Track};
use crate::library::query::{self, SearchResult};
use crate::library::scan::{lock, ScanReport};
use crate::library::watch::{FolderWatcher, WatchState};
use crate::library::{normalize, tauri_emitter, Library};
use crate::player::PlayerState;
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
    state.player.set_volume(v)
}

#[tauri::command]
pub fn player_state(state: State<'_, AppState>) -> Result<PlayerState, String> {
    Ok(state.player.state())
}

// Library

/// Adds a folder, indexes it and starts watching it. Folders inside or around an existing
/// library folder are refused.
#[tauri::command]
pub async fn library_add_folder(app: AppHandle, path: String) -> Result<ScanReport, String> {
    let folder = folder_path(&path)?;
    let library = library(&app);
    let emit_app = app.clone();
    let scanned = folder.clone();
    let report = blocking(move || library.add_folder(&scanned, &tauri_emitter(emit_app))).await?;
    with_watcher(&app, |watcher| watcher.watch(&folder));
    Ok(report)
}

/// Stops watching a folder and forgets it and its tracks.
#[tauri::command]
pub async fn library_remove_folder(app: AppHandle, path: String) -> Result<(), String> {
    let folder = folder_path(&path)?;
    let library = library(&app);
    let emit_app = app.clone();
    let removed = folder.clone();
    blocking(move || library.remove_folder(&removed, &tauri_emitter(emit_app))).await?;
    with_watcher(&app, |watcher| watcher.unwatch(&folder));
    Ok(())
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
