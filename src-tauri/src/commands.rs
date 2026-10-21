//! Tauri commands. Thin wrappers: the player and library services hold the logic. Every
//! command returns `Result<T, String>` with a message fit for the UI.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::audio::list_devices;
use crate::flow::{FlowOutcome, FlowVariant};
use crate::library::favorites::{self, FavKind, FavoriteIds};
use crate::library::model::{navidrome_raw, Album, Artist, Folder, Source, Track};
use crate::library::playlists::{self, Playlist, PlaylistEntry};
use crate::library::query::{self, SearchResult};
use crate::library::scan::{lock, ScanReport};
use crate::library::watch::{FolderWatcher, WatchState};
use crate::library::{normalize, tauri_emitter, Library};
use crate::navidrome::client::{NavidromeFailure, ServerInfo};
use crate::navidrome::playlists::{self as remote_playlists, RemotePlaylist};
use crate::navidrome::stars;
use crate::navidrome::{NavidromeService, NavidromeStatus, FAILURE_INTERNAL};
use crate::player::{PlayerState, QueueItem, DROPPED_PREFIX};
use crate::radio::directory::{Country, DirectoryError, Region, StationQuery, TagCount};
use crate::radio::location::Place;
use crate::radio::model::Station;
use crate::radio::{NearResult, NowPlaying, RadioService, Saved};
use crate::secret::Secret;
use crate::settings::{Change, Settings};
use crate::shell;
use crate::shell::mini::{self, MiniLayout};
use crate::shell::tray::TrayLabels;
use crate::ytm::{self, state::YtmState};
use crate::AppState;

/// Upper bound for `library_search` and `library_tracks_range` page sizes.
const MAX_PAGE: u32 = 1_000;

fn message(err: anyhow::Error) -> String {
    format!("{err:#}")
}

/// What a command answers when it arrives before setup managed the app state (a page that
/// loaded early).
const NOT_READY: &str = "the app state is not ready";

/// The app state, or an error for the UI instead of a panic when it is not managed yet.
fn app_state(app: &AppHandle) -> Result<State<'_, AppState>, String> {
    ready(app.try_state::<AppState>())
}

/// `app_state`'s rule, apart from the app (a mock runtime does not start in unit tests on
/// Windows).
fn ready<T>(state: Option<T>) -> Result<T, String> {
    state.ok_or_else(|| NOT_READY.to_string())
}

fn library(app: &AppHandle) -> Result<Arc<Library>, String> {
    Ok(Arc::clone(&app_state(app)?.library))
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
    let library = library(app)?;
    blocking(move || run(&*lock(&library.db)?)).await
}

/// Runs a write against the library database.
async fn write<T: Send + 'static>(
    app: &AppHandle,
    run: impl FnOnce(&mut Connection) -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    let library = library(app)?;
    blocking(move || run(&mut *lock(&library.db)?)).await
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

/// Refuses ids of dropped files ("Open with", drag and drop): they are not in the library,
/// so they cannot be favorites or playlist entries.
fn library_ids_only<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    match ids.into_iter().find(|id| id.starts_with(DROPPED_PREFIX)) {
        Some(id) => Err(format!(
            "\"{id}\" is a dropped file, not a library track: add its folder to the library first"
        )),
        None => Ok(()),
    }
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
    let dropped = Arc::clone(&app_state(&app)?.dropped);
    let reader = Arc::clone(&dropped);
    let tracks = blocking(move || Ok(reader.read(&paths))).await?;
    let state = app_state(&app)?;
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

/// Moves the queue entry at `from` to `to`; what plays keeps playing. Out of range is ignored.
#[tauri::command]
pub fn player_move(state: State<'_, AppState>, from: u32, to: u32) -> Result<(), String> {
    state.player.move_entry(from as usize, to as usize)
}

/// Removes the queue entry at `index`; removing the playing one plays the next one.
#[tauri::command]
pub fn player_remove(state: State<'_, AppState>, index: u32) -> Result<(), String> {
    state.player.remove(index as usize)
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
    blocking(move || {
        let state = app_state(&app).map_err(anyhow::Error::msg)?;
        Ok(state.player.queue_range(offset, limit))
    })
    .await
}

// Library

/// Adds a folder, indexes it and starts watching it. Folders inside or around an existing
/// library folder are refused.
#[tauri::command]
pub async fn library_add_folder(app: AppHandle, path: String) -> Result<ScanReport, String> {
    let folder = folder_path(&path)?;
    let library = library(&app)?;
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
    let library = library(&app)?;
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
    let library = library(&app)?;
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

/// Tracks in the index; `source` omitted counts every source.
#[tauri::command]
pub async fn library_track_count(app: AppHandle, source: Option<Source>) -> Result<u32, String> {
    read(&app, move |conn| query::track_count(conn, source)).await
}

/// A page of tracks in library order; `source` omitted pages through every source.
#[tauri::command]
pub async fn library_tracks_range(
    app: AppHandle,
    offset: u32,
    limit: u32,
    source: Option<Source>,
) -> Result<Vec<Track>, String> {
    let limit = limit.min(MAX_PAGE);
    read(&app, move |conn| {
        query::tracks_range(conn, offset, limit, source)
    })
    .await
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
    let flow = Arc::clone(&app_state(&app)?.flow);
    blocking(move || flow.start(variant)).await
}

// Favorites

/// Sets a favorite and returns the new state; emits `favorites://changed` when it changed. A
/// Navidrome item is also starred on the server (queued while it cannot be reached); that
/// fails with "Navidrome is not connected" while no account is signed in.
#[tauri::command]
pub async fn favorites_set(
    app: AppHandle,
    kind: FavKind,
    id: String,
    on: bool,
) -> Result<bool, String> {
    library_ids_only([id.as_str()])?;
    if kind == FavKind::Station {
        return Err("stations are favorited through radio_favorite".to_string());
    }
    let changed = if stars::target(kind, &id).is_some() {
        let navidrome = Arc::clone(&app_state(&app)?.navidrome);
        blocking(move || navidrome.set_favorite(kind, &id, on)).await?
    } else {
        write(&app, move |conn| {
            favorites::set_changed(conn, kind, &id, on)
        })
        .await?
    };
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
    let changed = read(&app, move |conn| {
        playlists::playlist_rename(conn, id, &name)
    })
    .await?;
    if changed {
        emit_playlists_changed(&app);
    }
    Ok(())
}

#[tauri::command]
pub async fn playlist_delete(app: AppHandle, id: i64) -> Result<(), String> {
    if read(&app, move |conn| playlists::playlist_delete(conn, id)).await? {
        emit_playlists_changed(&app);
    }
    Ok(())
}

#[tauri::command]
pub async fn playlist_entries(app: AppHandle, id: i64) -> Result<Vec<PlaylistEntry>, String> {
    read(&app, move |conn| playlists::playlist_entries(conn, id)).await
}

#[tauri::command]
pub async fn playlist_add(app: AppHandle, id: i64, track_ids: Vec<String>) -> Result<(), String> {
    library_ids_only(track_ids.iter().map(String::as_str))?;
    let changed = read(&app, move |conn| {
        playlists::playlist_add(conn, id, &track_ids)
    })
    .await?;
    if changed {
        emit_playlists_changed(&app);
    }
    Ok(())
}

#[tauri::command]
pub async fn playlist_remove(app: AppHandle, id: i64, positions: Vec<u32>) -> Result<(), String> {
    let changed = read(&app, move |conn| {
        playlists::playlist_remove(conn, id, &positions)
    })
    .await?;
    if changed {
        emit_playlists_changed(&app);
    }
    Ok(())
}

#[tauri::command]
pub async fn playlist_move(app: AppHandle, id: i64, from: u32, to: u32) -> Result<(), String> {
    let changed = read(&app, move |conn| {
        playlists::playlist_move(conn, id, from, to)
    })
    .await?;
    if changed {
        emit_playlists_changed(&app);
    }
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
    let notify = app.clone();
    let saved = blocking(move || {
        let state = app_state(&app).map_err(anyhow::Error::msg)?;
        // The shortcut and the start with the system go to the system first, outside the
        // settings lock (registering waits for the main thread), and are only saved when the
        // system took them; the error (invalid, taken, refused) goes to the UI as it is.
        let before = state.settings.get();
        let shortcut = patch.get("miniShortcut").and_then(|value| value.as_str());
        // Also applied when unchanged: that retries a shortcut the system refused at startup.
        if let Some(accelerator) = shortcut {
            shell::shortcut::apply(&app, accelerator).map_err(anyhow::Error::msg)?;
        }
        let registered = shortcut.is_some_and(|accelerator| accelerator != before.mini_shortcut);
        let autostart = patch
            .get("startWithSystem")
            .and_then(|value| value.as_bool())
            .filter(|on| *on != before.start_with_system);
        if let Some(on) = autostart {
            if let Err(err) = shell::shortcut::apply_start_with_system(&app, on) {
                if registered {
                    restore_shortcut(&app, &before.mini_shortcut);
                }
                return Err(anyhow::Error::msg(err));
            }
        }
        let saved = state.settings.update(patch, |changes| {
            for change in changes {
                let sent = match change {
                    Change::Volume(volume) => state.player.set_volume(*volume),
                    Change::OutputDevice(device) => state.player.set_device(device.clone()),
                    Change::Gapless(gapless) => state.player.set_gapless(*gapless),
                    Change::ReplayGain(mode) => state.player.set_replay_gain(*mode),
                    Change::Crossfade(mode, ms) => state.player.set_crossfade(*mode, *ms),
                    Change::Repeat(mode) => state.player.set_repeat(*mode),
                    Change::Shuffle(on) => state.player.set_shuffle(*on),
                    // Applied above, before the save.
                    Change::MiniShortcut(_) | Change::StartWithSystem(_) => Ok(()),
                    other => shell::apply_setting(&app, other),
                };
                if let Err(err) = sent {
                    eprintln!("applying a setting failed: {err}");
                }
            }
        });
        if saved.is_err() {
            // Not saved: the system goes back to what the file says.
            if registered {
                restore_shortcut(&app, &before.mini_shortcut);
            }
            if let Some(on) = autostart {
                if let Err(err) = shell::shortcut::apply_start_with_system(&app, !on) {
                    eprintln!("restoring the start with the system failed: {err}");
                }
            }
        }
        saved
    })
    .await?;
    // Every window follows the saved settings (the mini player's language, for one).
    if let Err(err) = notify.emit(crate::settings::EVENT_CHANGED, &saved) {
        eprintln!("emitting the settings change failed: {err}");
    }
    Ok(saved)
}

fn restore_shortcut(app: &AppHandle, accelerator: &str) {
    if let Err(err) = shell::shortcut::apply(app, accelerator) {
        eprintln!("restoring the previous shortcut failed: {err}");
    }
}

/// Names of the output devices, as `outputDevice` takes them.
#[tauri::command]
pub async fn audio_devices() -> Result<Vec<String>, String> {
    blocking(|| Ok(list_devices())).await
}

// Navidrome

fn navidrome(app: &AppHandle) -> Result<Arc<NavidromeService>, NavidromeFailure> {
    app_state(app)
        .map(|state| Arc::clone(&state.navidrome))
        .map_err(|_| NavidromeFailure {
            code: FAILURE_INTERNAL,
            detail: Some(NOT_READY.to_string()),
        })
}

/// Runs a blocking Navidrome call (network) off the async runtime's worker threads.
async fn navidrome_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, NavidromeFailure> + Send + 'static,
) -> Result<T, NavidromeFailure> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|err| NavidromeFailure {
            code: FAILURE_INTERNAL,
            detail: Some(format!("the Navidrome task failed: {err}")),
        })?
}

#[tauri::command]
pub fn navidrome_status(state: State<'_, AppState>) -> NavidromeStatus {
    state.navidrome.status()
}

/// Pings the server with these credentials; saves nothing.
#[tauri::command]
pub async fn navidrome_test(
    app: AppHandle,
    url: String,
    user: String,
    password: String,
) -> Result<ServerInfo, NavidromeFailure> {
    let service = navidrome(&app)?;
    let password = Secret::new(password);
    navidrome_blocking(move || {
        service
            .test(&url, &user, password)
            .map_err(|err| NavidromeFailure::from(&err))
    })
    .await
}

/// Signs in; the password goes to the OS keychain only.
#[tauri::command]
pub async fn navidrome_connect(
    app: AppHandle,
    url: String,
    user: String,
    password: String,
) -> Result<NavidromeStatus, NavidromeFailure> {
    let service = navidrome(&app)?;
    let password = Secret::new(password);
    navidrome_blocking(move || service.connect(&url, &user, password)).await
}

/// Signs out and removes the server's tracks, stars and playlists from the index.
#[tauri::command]
pub async fn navidrome_disconnect(app: AppHandle) -> Result<(), String> {
    let service = Arc::clone(&app_state(&app)?.navidrome);
    blocking(move || service.disconnect()).await
}

/// Runs a sync cycle now.
#[tauri::command]
pub fn navidrome_sync(state: State<'_, AppState>) -> Result<(), String> {
    state.navidrome.sync_now();
    Ok(())
}

/// The server's playlists in the index (read-only), most recently changed first.
#[tauri::command]
pub async fn navidrome_playlists(app: AppHandle) -> Result<Vec<RemotePlaylist>, String> {
    read(&app, remote_playlists::list).await
}

/// The entries of a server playlist; a track not in the index comes back with `track: None`.
#[tauri::command]
pub async fn navidrome_playlist_entries(
    app: AppHandle,
    id: String,
) -> Result<Vec<PlaylistEntry>, String> {
    if navidrome_raw(&id).is_none() {
        return Err("This playlist no longer exists".to_string());
    }
    read(&app, move |conn| remote_playlists::entries(conn, &id)).await
}

// Radio

/// A radio failure as the frontend sees it: a stable code, never a location or a host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RadioFailure {
    pub code: &'static str,
}

const RADIO_INTERNAL: &str = "internal";

/// The most results one search page may ask for.
const MAX_STATIONS: u32 = 200;

/// The search filters of the Radio view.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationQueryArgs {
    name: Option<String>,
    country_code: Option<String>,
    region: Option<String>,
    tag: Option<String>,
    offset: Option<u32>,
    limit: Option<u32>,
}

impl From<StationQueryArgs> for StationQuery {
    fn from(args: StationQueryArgs) -> StationQuery {
        let defaults = StationQuery::default();
        StationQuery {
            name: args.name,
            country_code: args.country_code,
            region: args.region,
            tag: args.tag,
            offset: args.offset.unwrap_or(0),
            limit: args.limit.unwrap_or(defaults.limit).clamp(1, MAX_STATIONS),
            ..defaults
        }
    }
}

fn radio(app: &AppHandle) -> Result<Arc<RadioService>, String> {
    Ok(Arc::clone(&app_state(app)?.radio))
}

/// Runs a blocking radio call (network) off the async runtime's worker threads.
async fn radio_blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|err| format!("the radio task failed: {err}"))
}

/// `radio_blocking` for the calls that fail with a directory error (`{ code }`).
async fn radio_directory<T: Send + 'static>(
    app: &AppHandle,
    work: impl FnOnce(&RadioService) -> Result<T, DirectoryError> + Send + 'static,
) -> Result<T, RadioFailure> {
    let internal = |_| RadioFailure {
        code: RADIO_INTERNAL,
    };
    let service = radio(app).map_err(internal)?;
    radio_blocking(move || work(&service))
        .await
        .map_err(internal)?
        .map_err(|err| RadioFailure { code: err.code() })
}

/// Where the user is for "near you"; looks it up on first use (automatic mode).
#[tauri::command]
pub async fn radio_place(app: AppHandle) -> Result<Place, String> {
    let service = radio(&app)?;
    radio_blocking(move || service.place()).await
}

/// Asks the location service again (automatic mode only) and answers the place.
#[tauri::command]
pub async fn radio_redetect(app: AppHandle) -> Result<Place, String> {
    let service = radio(&app)?;
    radio_blocking(move || service.redetect()).await
}

/// Stations near the user; `refresh` skips the cache.
#[tauri::command]
pub async fn radio_near(app: AppHandle, refresh: bool) -> Result<NearResult, String> {
    let service = radio(&app)?;
    radio_blocking(move || service.near(refresh)).await
}

#[tauri::command]
pub async fn radio_search(
    app: AppHandle,
    query: StationQueryArgs,
) -> Result<Vec<Station>, RadioFailure> {
    let query = StationQuery::from(query);
    radio_directory(&app, move |service| service.search(query)).await
}

#[tauri::command]
pub async fn radio_countries(app: AppHandle) -> Result<Vec<Country>, RadioFailure> {
    radio_directory(&app, |service| service.countries()).await
}

#[tauri::command]
pub async fn radio_regions(
    app: AppHandle,
    country_code: String,
) -> Result<Vec<Region>, RadioFailure> {
    radio_directory(&app, move |service| service.regions(&country_code)).await
}

#[tauri::command]
pub async fn radio_tags(app: AppHandle) -> Result<Vec<TagCount>, RadioFailure> {
    radio_directory(&app, |service| service.top_tags()).await
}

/// The favorite stations and the recently played ones.
#[tauri::command]
pub async fn radio_saved(app: AppHandle) -> Result<Saved, String> {
    let service = radio(&app)?;
    radio_blocking(move || service.saved())
        .await?
        .map_err(message)
}

/// Stores the station, reports the click and plays it as a queue of one.
#[tauri::command]
pub async fn radio_play(app: AppHandle, station: Station) -> Result<(), String> {
    let service = radio(&app)?;
    let id = radio_blocking(move || service.play_prepared(station))
        .await?
        .map_err(message)?;
    app_state(&app)?.player.play(vec![id], 0)
}

/// Sets a station favorite and returns the new state. Stations go through here, not
/// `favorites_set`: the station itself must be stored with its `radio` source.
#[tauri::command]
pub async fn radio_favorite(app: AppHandle, station: Station, on: bool) -> Result<bool, String> {
    let service = radio(&app)?;
    radio_blocking(move || service.set_favorite(station, on).map(|_| on))
        .await?
        .map_err(message)
}

/// Opens a station's homepage in the browser. The link comes from the service's own data
/// (never from the caller) and only http(s) is opened.
#[tauri::command]
pub async fn radio_open_homepage(app: AppHandle, station_id: String) -> Result<(), String> {
    let service = radio(&app)?;
    let url = radio_blocking(move || service.homepage_url(&station_id))
        .await?
        .map_err(message)?;
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|err| err.to_string())
}

/// The station that is current and its stream title, for windows that load later.
#[tauri::command]
pub fn radio_now_playing(state: State<'_, AppState>) -> Option<NowPlaying> {
    state.radio.now_playing()
}

// Shell

/// Shows the mini player (bottom-right, or under the menu bar icon on macOS, without taking
/// focus), or hides it.
#[tauri::command]
pub fn mini_toggle(app: AppHandle) -> Result<(), String> {
    mini::toggle(&app, &mini::home_anchor(&app)).map_err(|err| err.to_string())
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

/// Brings the main window up (shown with its taskbar button, restored and focused) and hides
/// the mini player.
#[tauri::command]
pub fn main_show(app: AppHandle) -> Result<(), String> {
    shell::main_show(&app)
}

/// The tray menu's strings in the UI language, sent by the main window.
#[tauri::command]
pub fn tray_set_labels(app: AppHandle, labels: TrayLabels) {
    shell::tray::set_labels(&app, &labels);
}

// YouTube Music.

const YTM_REFUSED: &str = "refused";

/// Where the YouTube Music view goes in the main window, in logical pixels.
pub use crate::ytm::view::Rect as YtmRect;

/// The one command the remote YouTube Music page may call. Only the `ytm` view on
/// music.youtube.com is heard; the answer never says more than "refused".
#[tauri::command]
pub fn ytm_report(webview: tauri::Webview, report: serde_json::Value) -> Result<(), String> {
    let url = webview.url().ok();
    if !ytm::report_allowed(webview.label(), url.as_ref(), ytm::dev_host()) {
        return Err(YTM_REFUSED.to_string());
    }
    let report = ytm::report::parse(report).map_err(|_| YTM_REFUSED.to_string())?;
    app_state(webview.app_handle())?.ytm.handle(report);
    Ok(())
}

/// Shows the view over the page's placeholder (created on the first call). Sync, so a show
/// and a hide run in the order the page sent them.
#[tauri::command]
pub fn ytm_show(state: State<'_, AppState>, rect: YtmRect) -> Result<(), String> {
    state.ytm_view.show(rect)
}

/// Hides the view; YouTube Music keeps playing.
#[tauri::command]
pub fn ytm_hide(state: State<'_, AppState>) -> Result<(), String> {
    state.ytm_view.hide()
}

/// A command for the YouTube Music page itself, whichever player is active:
/// `invoke('ytm_control', { action })` with `toggle`, `play`, `pause`, `next` or `prev`;
/// `{ action: 'seek', positionMs }`; `{ action: 'volume', volume }` (0..=1).
#[tauri::command]
pub fn ytm_control(
    state: State<'_, AppState>,
    action: String,
    position_ms: Option<f64>,
    volume: Option<f64>,
) -> Result<(), String> {
    let action = ytm::parse_action(&action, position_ms, volume)?;
    ytm::state::YtmControl::send(state.ytm.as_ref(), action);
    Ok(())
}

#[tauri::command]
pub fn ytm_state(state: State<'_, AppState>) -> Result<YtmState, String> {
    Ok(state.ytm.state())
}

/// Opens Google's sign-in in its own window, or brings it to the front.
#[tauri::command]
pub fn ytm_sign_in(state: State<'_, AppState>) -> Result<(), String> {
    state.ytm_view.sign_in();
    Ok(())
}

/// Forgets the Google session and the view.
#[tauri::command]
pub async fn ytm_sign_out(app: AppHandle) -> Result<(), String> {
    let view = Arc::clone(&app_state(&app)?.ytm_view);
    tauri::async_runtime::spawn_blocking(move || view.sign_out())
        .await
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command that arrives before setup managed the app state gets an error, not a panic.
    #[test]
    fn app_state_is_an_error_until_managed() {
        assert_eq!(ready::<u8>(None), Err(NOT_READY.to_string()));
        assert_eq!(ready(Some(7)), Ok(7));
    }

    /// Every radio command is in the handler list in `lib.rs`.
    #[test]
    fn radio_commands_are_registered() {
        let lib = include_str!("lib.rs");
        for name in [
            "radio_place",
            "radio_near",
            "radio_search",
            "radio_countries",
            "radio_regions",
            "radio_tags",
            "radio_saved",
            "radio_play",
            "radio_favorite",
            "radio_now_playing",
            "radio_open_homepage",
            "radio_redetect",
        ] {
            assert!(
                lib.contains(&format!("commands::{name},")),
                "{name} is not registered"
            );
        }
    }

    #[test]
    fn search_args_are_clamped_and_filled() {
        let args: StationQueryArgs =
            serde_json::from_str(r#"{"name":"jazz","countryCode":"DE","limit":5000}"#).unwrap();
        let query = StationQuery::from(args);
        assert_eq!(query.name.as_deref(), Some("jazz"));
        assert_eq!(query.country_code.as_deref(), Some("DE"));
        assert_eq!(query.limit, MAX_STATIONS);
        assert_eq!(query.offset, 0);
        let empty = StationQuery::from(StationQueryArgs::default());
        assert_eq!(empty.limit, StationQuery::default().limit);
        let zero: StationQueryArgs = serde_json::from_str(r#"{"limit":0}"#).unwrap();
        assert_eq!(StationQuery::from(zero).limit, 1);
    }

    #[test]
    fn dropped_files_are_refused_as_favorites_and_playlist_entries() {
        assert_eq!(library_ids_only(["abc", "def"]), Ok(()));
        assert_eq!(library_ids_only([]), Ok(()));
        let dropped = format!("{DROPPED_PREFIX}0");
        let err = library_ids_only(["abc", dropped.as_str()]).unwrap_err();
        assert!(err.contains(&dropped), "{err}");
        assert!(err.contains("dropped file"), "{err}");
    }
}
