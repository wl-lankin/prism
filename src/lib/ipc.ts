import { invoke } from '@tauri-apps/api/core';
import { emitTo, listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  Album,
  Artist,
  FavKind,
  FavoriteIds,
  FlowOutcome,
  FlowVariant,
  Folder,
  NavidromeStatus,
  PlayerError,
  PlayerState,
  Playlist,
  PlaylistEntry,
  QueueItem,
  RemotePlaylist,
  ScanProgress,
  ScanReport,
  SearchResult,
  ServerInfo,
  Settings,
  Source,
  Track,
} from './types';

export type CoverSize = 96 | 300 | 600;

/** Windows (WebView2) and Android serve custom schemes as `http://<scheme>.localhost`; others keep `<scheme>://localhost`. */
function isWindows(): boolean {
  return /Windows|Android/.test(navigator.userAgent);
}

export function coverUrl(id: string | null, size: CoverSize): string | null {
  if (!id) return null;
  const base = isWindows() ? 'http://prism.localhost' : 'prism://localhost';
  return `${base}/cover/${encodeURIComponent(id)}/${size}`;
}

function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, args);
}

// Player commands. They return once the engine has the command, not when it is done;
// the result arrives as `player://state`.

export const playerPlay = (trackIds: string[], startIndex: number) =>
  call<void>('player_play', { trackIds, startIndex });
export const playerToggle = () => call<void>('player_toggle');
export const playerNext = () => call<void>('player_next');
/** Restarts the track after 3 s, otherwise goes to the previous one. */
export const playerPrev = () => call<void>('player_prev');
export const playerSeek = (ms: number) => call<void>('player_seek', { ms: Math.max(0, Math.round(ms)) });
/** 0..1 */
export const playerSetVolume = (v: number) => call<void>('player_set_volume', { v });
export const playerState = () => call<PlayerState>('player_state');
/** Plays files (dropped onto the window) without indexing them. Folders and non-audio files
 *  are skipped. Resolves with how many files were queued; 0 leaves the player untouched. */
export const playerPlayFiles = (paths: string[]) => call<number>('player_play_files', { paths });
/** Adds tracks at the end of the queue; what plays now keeps playing. */
export const playerAppend = (trackIds: string[]) => call<void>('player_append', { trackIds });
/** Plays the queue entry at `index`; an index out of range is ignored. */
export const playerJump = (index: number) => call<void>('player_jump', { index });
/** Up to `limit` queue entries from `offset` (`limit` is capped at 1000). */
/** Moves the queue entry at `from` to `to`; what plays keeps playing. */
export const playerMove = (from: number, to: number) => call<void>('player_move', { from, to });
/** Removes the queue entry at `index`; removing the playing one plays the next one. */
export const playerRemove = (index: number) => call<void>('player_remove', { index });
export const playerQueueRange = (offset: number, limit: number) =>
  call<QueueItem[]>('player_queue_range', { offset, limit });

// Library commands.

/** Refused (rejects with a message) when the folder is inside or contains a library folder. */
export const libraryAddFolder = (path: string) => call<ScanReport>('library_add_folder', { path });
export const libraryRemoveFolder = (path: string) => call<void>('library_remove_folder', { path });
export const libraryFolders = () => call<Folder[]>('library_folders');
export const libraryRescan = () => call<void>('library_rescan');
/** Sorted by artist, year, title. */
export const libraryAlbums = () => call<Album[]>('library_albums');
export const libraryAlbumTracks = (albumId: string) => call<Track[]>('library_album_tracks', { albumId });
export const libraryArtists = () => call<Artist[]>('library_artists');
export const libraryArtistAlbums = (name: string) => call<Album[]>('library_artist_albums', { name });
/** Tracks in the index; without a source, of every source. */
export const libraryTrackCount = (source?: Source) =>
  call<number>('library_track_count', source ? { source } : undefined);
/** Sorted by artist, album, disc, track; without a source, through every source. */
export const libraryTracksRange = (offset: number, limit: number, source?: Source) =>
  call<Track[]>('library_tracks_range', source ? { offset, limit, source } : { offset, limit });
export const librarySearch = (q: string, limit: number) => call<SearchResult>('library_search', { q, limit });
/** Every genre once, blank ones left out, sorted case-insensitively. */
export const libraryGenres = () => call<string[]>('library_genres');

// Flow.

/** Plays a new endless Flow; favorites without any fall back to the whole library. */
export const flowStart = (variant: FlowVariant) => call<FlowOutcome>('flow_start', { variant });

// Favorites.

/** Idempotent; resolves to the new state. */
export const favoritesSet = (kind: FavKind, id: string, on: boolean) =>
  call<boolean>('favorites_set', { kind, id, on });
export const favoritesIds = () => call<FavoriteIds>('favorites_ids');
/** Only items that are in the index; newest favorite first. */
export const favoritesTracks = () => call<Track[]>('favorites_tracks');
export const favoritesAlbums = () => call<Album[]>('favorites_albums');
export const favoritesArtists = () => call<Artist[]>('favorites_artists');

// Playlists.

export const playlistsList = () => call<Playlist[]>('playlists_list');
export const playlistCreate = (name: string) => call<Playlist>('playlist_create', { name });
export const playlistRename = (id: number, name: string) => call<void>('playlist_rename', { id, name });
export const playlistDelete = (id: number) => call<void>('playlist_delete', { id });
export const playlistEntries = (id: number) => call<PlaylistEntry[]>('playlist_entries', { id });
export const playlistAdd = (id: number, trackIds: string[]) => call<void>('playlist_add', { id, trackIds });
export const playlistRemove = (id: number, positions: number[]) =>
  call<void>('playlist_remove', { id, positions });
export const playlistMove = (id: number, from: number, to: number) =>
  call<void>('playlist_move', { id, from, to });

// Navidrome. `navidromeTest` and `navidromeConnect` reject with a `NavidromeFailure`; the
// other commands reject with a message.

export const navidromeStatus = () => call<NavidromeStatus>('navidrome_status');
/** Pings the server with these credentials; saves nothing. */
export const navidromeTest = (url: string, user: string, password: string) =>
  call<ServerInfo>('navidrome_test', { url, user, password });
/** Signs in; the password goes to the OS keychain only. Resolves with the new status. */
export const navidromeConnect = (url: string, user: string, password: string) =>
  call<NavidromeStatus>('navidrome_connect', { url, user, password });
/** Signs out and removes the server's tracks, stars and playlists from the index. */
export const navidromeDisconnect = () => call<void>('navidrome_disconnect');
/** Starts a sync cycle now; progress arrives as `navidrome://status`. */
export const navidromeSync = () => call<void>('navidrome_sync');
/** The server's playlists in the index, most recently changed first. */
export const navidromePlaylists = () => call<RemotePlaylist[]>('navidrome_playlists');
/** The entries of a server playlist; a track not in the index comes back with `track: null`. */
export const navidromePlaylistEntries = (id: string) =>
  call<PlaylistEntry[]>('navidrome_playlist_entries', { id });

// Settings.

export const settingsGet = () => call<Settings>('settings_get');
/** Merges `patch` into the settings, saves them and applies them to the player at once.
 *  Resolves with the full settings. */
export const settingsSet = (patch: Partial<Settings>) => call<Settings>('settings_set', { patch });
/** Output device names, as `outputDevice` takes them. */
export const audioDevices = () => call<string[]>('audio_devices');

// Window.

/** True when the window sits on a native glass material (Mica, macOS vibrancy). */
export const windowVibrancy = () => call<boolean>('window_vibrancy');
/** The one-time startup notice (a quarantined library file name), cleared once read. */
export const startupNotice = () => call<string | null>('startup_notice');

// Mini player and main window.

/** The mini player's window label; `main.ts` mounts the mini player for it. */
export const MINI_LABEL = 'mini';

/** Where the player sits in the mini window: `top` logical px down (the Flow menu's room
 *  is above it when that is not 0). */
export interface MiniLayout {
  top: number;
}

/** Shows the mini player bottom-right without taking focus, or hides it. */
export const miniToggle = () => call<void>('mini_toggle');
export const miniLayout = () => call<MiniLayout>('mini_layout');
/** The mini player's Flow menu opened or closed (the window's shape follows). */
export const miniMenu = (open: boolean) => call<void>('mini_menu', { open });
/** Shows, restores and focuses the main window; hides the mini player. */
export const mainShow = () => call<void>('main_show');

/** The tray menu's (and the Windows thumbnail buttons') strings, in the UI language. */
export interface TrayLabels {
  open: string;
  playPause: string;
  next: string;
  previous: string;
  quit: string;
}

/** Sends the tray strings; the main window does it whenever the language resolves or changes. */
export const traySetLabels = (labels: TrayLabels) => call<void>('tray_set_labels', { labels });

/** A place for the main window to go to (`nav://go`), sent from the mini player. */
export interface NavTarget {
  view: 'nowplaying';
  param?: string;
}

/** Tells the main window to show `target`. */
export const navGoMain = (target: NavTarget) => emitTo('main', 'nav://go', target);

// Events.

export const onPlayerState = (cb: (state: PlayerState) => void): Promise<UnlistenFn> =>
  listen<PlayerState>('player://state', (event) => cb(event.payload));
/** 0..1, the loudness of what plays, every 50 ms while playing. */
export const onPlayerLevel = (cb: (level: number) => void): Promise<UnlistenFn> =>
  listen<number>('player://level', (event) => cb(event.payload));
export const onPlayerError = (cb: (error: PlayerError) => void): Promise<UnlistenFn> =>
  listen<PlayerError>('player://error', (event) => cb(event.payload));
export const onLibraryChanged = (cb: () => void): Promise<UnlistenFn> => listen('library://changed', () => cb());
export const onFavoritesChanged = (cb: () => void): Promise<UnlistenFn> =>
  listen('favorites://changed', () => cb());
export const onPlaylistsChanged = (cb: () => void): Promise<UnlistenFn> =>
  listen('playlists://changed', () => cb());
export const onNavGo = (cb: (target: NavTarget) => void): Promise<UnlistenFn> =>
  listen<NavTarget>('nav://go', (event) => cb(event.payload));
/** The full settings after any window saved a change. */
export const onSettingsChanged = (cb: (settings: Settings) => void): Promise<UnlistenFn> =>
  listen<Settings>('settings://changed', (event) => cb(event.payload));
/** The mini player was just shown (it never gets focus or a visibility change for it). */
export const onMiniShown = (cb: () => void): Promise<UnlistenFn> => listen('mini://shown', () => cb());
export const onMiniLayout = (cb: (layout: MiniLayout) => void): Promise<UnlistenFn> =>
  listen<MiniLayout>('mini://layout', (event) => cb(event.payload));
/** The Navidrome status after every change (state, counts, sync progress, failure). */
export const onNavidromeStatus = (cb: (status: NavidromeStatus) => void): Promise<UnlistenFn> =>
  listen<NavidromeStatus>('navidrome://status', (event) => cb(event.payload));
export const onScanProgress =(cb: (progress: ScanProgress) => void): Promise<UnlistenFn> =>
  listen<ScanProgress>('library://scan-progress', (event) => cb(event.payload));
