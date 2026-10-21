// Shapes of everything the Rust backend sends (serde camelCase).

export type Source = 'local' | 'navidrome' | 'radio';

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

/** An artist of one source. `id` is the name for local artists, `navidrome:<id>` for Navidrome. */
export interface Artist {
  id: string;
  source: Source;
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

/** `station` hearts go through `radio_favorite`, not `favorites_set`. */
export type FavKind = 'track' | 'album' | 'artist' | 'station';

/** Every favorite id, newest first, whether or not the item is in the index. */
export interface FavoriteIds {
  tracks: string[];
  albums: string[];
  artists: string[];
  stations: string[];
}

/** An internet radio station as the backend sends it. Its player id is `radio:<uuid>`. */
export interface Station {
  id: string;
  name: string;
  url: string;
  urlResolved: string | null;
  homepage: string | null;
  favicon: string | null;
  coverId: string | null;
  countryCode: string | null;
  region: string | null;
  language: string | null;
  tags: string[];
  codec: string | null;
  bitrateKbps: number | null;
  hls: boolean;
  lastCheckOk: boolean;
  distanceKm: number | null;
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
  /** The Flow the queue was dealt for; null outside Flow. */
  flow?: FlowVariant | null;
  /** The chosen repeat mode; a Flow ignores it. */
  repeat?: RepeatMode;
  /** The upcoming part plays in random order; a Flow ignores it. */
  shuffle?: boolean;
}

/** Off, all (the first track follows the last), one (the track plays again). */
export type RepeatMode = 'off' | 'all' | 'one';

/** Where a Flow draws its tracks from. */
export type FlowVariant = { kind: 'favorites' } | { kind: 'library' } | { kind: 'genre'; genre: string };

/** What `flow_start` did; favorites without any fall back to the library (`variant`). */
export type FlowOutcome = { outcome: 'started'; variant: FlowVariant } | { outcome: 'empty' };

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
  repeat: RepeatMode;
  shuffle: boolean;
  /** The connected Navidrome server; only connect / disconnect change it. */
  navidromeUrl: string | null;
  /** The connected Navidrome user; only connect / disconnect change it. */
  navidromeUser: string | null;
  navidromeQuality: StreamQuality;
  radioLocation: RadioLocation;
}

/** Where "Radio near you" looks: an IP lookup, or a country (and region) the user chose. */
export type RadioLocation = { mode: 'auto' } | { mode: 'manual'; countryCode: string; region: string | null };

/** Navidrome stream quality: the original file or an MP3 transcode at a bitrate cap. */
export type StreamQuality = 'original' | '320' | '192' | '128';

/** Where the Navidrome account stands. `error` needs the user (credentials, TLS, a refused pass). */
export type LinkState = 'off' | 'syncing' | 'online' | 'offline' | 'error';

/** Every failure code the backend sends: the Subsonic client's nine, then the service's own. */
export type NavidromeErrorCode =
  | 'invalidUrl'
  | 'wrongCredentials'
  | 'tokenAuthUnsupported'
  | 'notAuthorized'
  | 'notFound'
  | 'unreachable'
  | 'tls'
  | 'notSubsonic'
  | 'server'
  | 'syncRefused'
  | 'keychain'
  | 'keychainTooLong'
  | 'database'
  | 'settings'
  | 'internal';

/** What `navidrome_test` and `navidrome_connect` reject with, and `NavidromeStatus.error`.
 *  `detail` is English technical text from the backend, never a credential. */
export interface NavidromeFailure {
  code: NavidromeErrorCode;
  detail: string | null;
}

/** Songs written so far in the song pass; `total` is the albums' song count sum, when known. */
export interface SyncProgress {
  done: number;
  total: number | null;
}

/** `navidrome_status` and the `navidrome://status` event. */
export interface NavidromeStatus {
  /** An account is saved (url and user). */
  configured: boolean;
  url: string | null;
  user: string | null;
  state: LinkState;
  /** Ms since the epoch of the last complete sync. */
  lastSync: number | null;
  /** Navidrome tracks and albums in the index. */
  trackCount: number;
  albumCount: number;
  progress: SyncProgress | null;
  error: NavidromeFailure | null;
}

/** What `navidrome_test` tells about the server. */
export interface ServerInfo {
  serverType: string | null;
  serverVersion: string | null;
  apiVersion: string;
  openSubsonic: boolean;
}

/** A playlist of the server (read-only), as `navidrome_playlists` lists it. */
export interface RemotePlaylist {
  /** `navidrome:<playlist id>`. */
  id: string;
  name: string;
  owner: string | null;
  /** The server's song count. */
  trackCount: number;
  /** The server's total duration. */
  durationMs: number;
  /** Up to four distinct cover ids of indexed tracks, in playlist order. */
  coverIds: string[];
  /** The server's `changed` time (ms), or when Prism first stored the playlist. */
  updatedAt: number;
}

/** One entry of `player_queue_range`; `track` is null when the id no longer resolves. */
export interface QueueItem {
  index: number;
  track: Track | null;
}

/** `track`: the track could not be played and the player moved on. `offline`: the server or
 * the station could not be reached; the player paused on the entry and Play resumes it where it
 * stopped (a station joins live again). */
export type PlayerErrorKind = 'track' | 'offline';

export interface PlayerError {
  trackId: string;
  message: string;
  kind: PlayerErrorKind;
  /** Picks the offline notice: `navidrome.offlineNotice` or `radio.offlineNotice`. */
  source: Source;
}

/** Where the user is, as coarsely as "Radio near you" needs it. */
export interface Place {
  countryCode: string;
  region: string | null;
  city: string | null;
  lat: number | null;
  lon: number | null;
  origin: 'detected' | 'manual' | 'locale';
  detectedAt: number | null;
}

/** Why the radio directory could not be asked (`radio_*` commands reject with `{ code }`). */
export type RadioErrorCode = 'unreachable' | 'server' | 'badResponse' | 'internal';

export interface RadioFailure {
  code: RadioErrorCode;
}

export interface NearResult {
  stations: Station[];
  place: Place;
  /** The list comes from the cache. */
  cached: boolean;
  /** The cached list is older than 6 hours. */
  stale: boolean;
  error: RadioErrorCode | null;
}

export interface SavedStations {
  favorites: Station[];
  recent: Station[];
}

export interface NowPlaying {
  stationId: string;
  title: string | null;
}

export interface StationQueryArgs {
  name?: string;
  countryCode?: string;
  region?: string;
  tag?: string;
  offset?: number;
  limit?: number;
}

export interface RadioCountry {
  code: string;
  name: string;
  stationCount: number;
}

export interface RadioRegion {
  name: string;
  stationCount: number;
}

export interface RadioTag {
  name: string;
  stationCount: number;
}
