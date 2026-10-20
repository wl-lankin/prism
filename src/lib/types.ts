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
}

export interface PlayerError {
  trackId: string;
  message: string;
}
