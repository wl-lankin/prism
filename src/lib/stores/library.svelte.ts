import { libraryAlbums, libraryArtists, libraryTrackCount, onLibraryChanged, onScanProgress } from '../ipc';
import type { Album, Artist, ScanProgress } from '../types';

/** The library as the views see it. Lists are raw state: they are replaced, never mutated,
 *  so tens of thousands of entries cost no deep proxies. */
class LibraryStore {
  /** null until the first load. */
  albums = $state.raw<Album[] | null>(null);
  artists = $state.raw<Artist[] | null>(null);
  trackCount = $state<number | null>(null);
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

/** Reloads albums, artists and the track count. Calls during a reload queue one more. */
export function reloadLibrary(): Promise<void> {
  if (loading) {
    again = true;
    return loading;
  }
  loading = (async () => {
    try {
      do {
        again = false;
        const [albums, artists, count] = await Promise.all([libraryAlbums(), libraryArtists(), libraryTrackCount()]);
        library.albums = albums ?? [];
        library.artists = artists ?? [];
        library.trackCount = count ?? 0;
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
