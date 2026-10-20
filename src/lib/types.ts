// Shapes of everything the Rust backend sends (serde camelCase).

export type Source = 'local' | 'navidrome';

export interface Track {
  id: string;
  source: Source;
  title: string;
  artist: string;
  albumArtist: string;
  album: string;
  albumId: string;
  trackNo: number | null;
  discNo: number | null;
  durationMs: number;
  year: number | null;
  genre: string | null;
  format: string;
  sampleRate: number | null;
  bitDepth: number | null;
  bitrateKbps: number | null;
  coverId: string | null;
  path: string | null;
  replayGainTrackDb: number | null;
  replayGainAlbumDb: number | null;
  replayGainTrackPeak: number | null;
  replayGainAlbumPeak: number | null;
}

export interface Album {
  id: string;
  source: Source;
  title: string;
  artist: string;
  year: number | null;
  coverId: string | null;
  trackCount: number;
  durationMs: number;
}

export interface Artist {
  name: string;
  albumCount: number;
  trackCount: number;
  coverId: string | null;
}

export interface Folder {
  path: string;
  online: boolean;
  trackCount: number;
}

export interface SearchResult {
  albums: Album[];
  artists: Artist[];
  tracks: Track[];
}

export type FavKind = 'track' | 'album' | 'artist';

/** Every favorite id, newest first, whether or not the item is in the index. */
export interface FavoriteIds {
  tracks: string[];
  albums: string[];
  artists: string[];
}

export interface Playlist {
  id: number;
  name: string;
  /** Entries, including those whose track is not in the index. */
  trackCount: number;
  durationMs: number;
  /** Up to four distinct cover ids, for a mosaic. */
  coverIds: string[];
  updatedAt: number;
}

export interface PlaylistEntry {
  position: number;
  trackId: string;
  /** Null when the track is not in the index (shown dimmed, skipped when playing). */
  track: Track | null;
}

export interface ScanReport {
  added: number;
  updated: number;
  removed: number;
  unchanged: number;
  /** [file, message] pairs */
  failed: [string, string][];
}

export interface ScanProgress {
  folder: string;
  done: number;
  total: number;
}

export type PlayerStatus = 'idle' | 'loading' | 'playing' | 'paused';

export interface PlayerState {
  status: PlayerStatus;
  track: Track | null;
  positionMs: number;
  durationMs: number;
  volume: number;
  queueIndex: number;
  queueLength: number;
  /** Bumped whenever the queue's ids change (not when only the position moves). */
  queueVersion: number;
}

export type LanguagePref = 'system' | 'en' | 'de';
export type ReplayGainMode = 'off' | 'track' | 'album';
export type CrossfadeMode = 'off' | 'flow' | 'always';

/** `<appConfig>/settings.json`, as `settings_get` / `settings_set` return it. */
export interface Settings {
  language: LanguagePref;
  /** 0..1 */
  volume: number;
  /** A name from `audio_devices`; null is the system default. */
  outputDevice: string | null;
  gapless: boolean;
  replayGain: ReplayGainMode;
  crossfade: CrossfadeMode;
  /** 1000..8000 */
  crossfadeMs: number;
  coverLookup: boolean;
  miniOnTop: boolean;
  /** A global shortcut in Tauri's accelerator syntax. */
  miniShortcut: string;
  startWithSystem: boolean;
}

/** One entry of `player_queue_range`; `track` is null when the id no longer resolves. */
export interface QueueItem {
  index: number;
  track: Track | null;
}

export interface PlayerError {
  trackId: string;
  message: string;
}
