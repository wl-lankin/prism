import { libraryAlbums, libraryArtists, libraryTrackCount, onLibraryChanged, onScanProgress } from '../ipc';
import type { Album, Artist, ScanProgress, Source } from '../types';

/** Tracks in the index, of every source and per source. */
export interface TrackCounts {
  all: number;
  local: number;
  navidrome: number;
}

/** The library as the views see it. Lists are raw state: they are replaced, never mutated,
 *  so tens of thousands of entries cost no deep proxies. Albums and artists hold every
 *  source; `albumsOf` / `artistsOf` narrow them. */
class LibraryStore {
  /** null until the first load. */
  albums = $state.raw<Album[] | null>(null);
  artists = $state.raw<Artist[] | null>(null);
  trackCounts = $state.raw<TrackCounts | null>(null);
  /** Bumped on every reload, so paged views drop their caches. */
  version = $state(0);
  /** The running scan, if any. */
  scan = $state.raw<ScanProgress | null>(null);
  /** Set when loading failed. */
  error = $state<string | null>(null);
}

export const library = new LibraryStore();

let loading: Promise<void> | null = null;
let again = false;

/** The albums of one source, or of every source. Empty before the first load. */
export function albumsOf(source?: Source): Album[] {
  const albums = library.albums ?? [];
  return source ? albums.filter((album) => album.source === source) : albums;
}

/** The artists of one source, or of every source. Empty before the first load. */
export function artistsOf(source?: Source): Artist[] {
  const artists = library.artists ?? [];
  return source ? artists.filter((artist) => artist.source === source) : artists;
}

/** True when more than one source has tracks: only then do mixed lists mark each item with
 *  its source (a library of local files alone looks as it always did). */
export function hasMixedSources(): boolean {
  const counts = library.trackCounts;
  return !!counts && counts.local > 0 && counts.navidrome > 0;
}

/** Reloads albums, artists and the track counts. Calls during a reload queue one more. */
export function reloadLibrary(): Promise<void> {
  if (loading) {
    again = true;
    return loading;
  }
  loading = (async () => {
    try {
      do {
        again = false;
        const [albums, artists, local, navidrome] = await Promise.all([
          libraryAlbums(),
          libraryArtists(),
          libraryTrackCount('local'),
          libraryTrackCount('navidrome'),
        ]);
        library.albums = albums ?? [];
        library.artists = artists ?? [];
        // `all` is the sum, so the three counts always agree (no third read in between).
        const counts = { local: local ?? 0, navidrome: navidrome ?? 0 };
        library.trackCounts = { all: counts.local + counts.navidrome, ...counts };
        library.error = null;
        library.version++;
      } while (again);
    } catch (err) {
      library.error = String(err);
    } finally {
      loading = null;
    }
  })();
  return loading;
}

/** Loads the library and follows `library://changed` and scan progress. Returns the cleanup. */
export function initLibrary(): () => void {
  const stops: (() => void)[] = [];
  let gone = false;
  const keep = (stop: () => void) => (gone ? stop() : stops.push(stop));
  let clearScan: ReturnType<typeof setTimeout> | undefined;

  onLibraryChanged(() => {
    library.scan = null;
    void reloadLibrary();
  })
    .then(keep)
    .catch(() => {});
  onScanProgress((progress) => {
    clearTimeout(clearScan);
    // 0 of 0 means there was nothing to read (or the scan ended early): done, no bar.
    if (progress.total === 0) {
      library.scan = null;
      return;
    }
    library.scan = progress;
    // A finished scan that changed nothing sends no `changed`; let the bar go by itself.
    if (progress.done >= progress.total) clearScan = setTimeout(() => (library.scan = null), 1500);
  })
    .then(keep)
    .catch(() => {});
  void reloadLibrary();

  return () => {
    gone = true;
    clearTimeout(clearScan);
    for (const stop of stops) stop();
  };
}
