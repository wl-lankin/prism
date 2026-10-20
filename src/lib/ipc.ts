import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  Album,
  Artist,
  Folder,
  PlayerError,
  PlayerState,
  ScanProgress,
  ScanReport,
  SearchResult,
  Settings,
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
export const libraryTrackCount = () => call<number>('library_track_count');
/** Sorted by artist, album, disc, track. */
export const libraryTracksRange = (offset: number, limit: number) =>
  call<Track[]>('library_tracks_range', { offset, limit });
export const librarySearch = (q: string, limit: number) => call<SearchResult>('library_search', { q, limit });

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

// Events.

export const onPlayerState = (cb: (state: PlayerState) => void): Promise<UnlistenFn> =>
  listen<PlayerState>('player://state', (event) => cb(event.payload));
export const onPlayerError = (cb: (error: PlayerError) => void): Promise<UnlistenFn> =>
  listen<PlayerError>('player://error', (event) => cb(event.payload));
export const onLibraryChanged = (cb: () => void): Promise<UnlistenFn> => listen('library://changed', () => cb());
export const onScanProgress = (cb: (progress: ScanProgress) => void): Promise<UnlistenFn> =>
  listen<ScanProgress>('library://scan-progress', (event) => cb(event.payload));
