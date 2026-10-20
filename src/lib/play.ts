import { t } from './i18n';
import { libraryAlbumTracks, libraryTracksRange, playerPlay } from './ipc';
import { shuffle } from './flow';
import { notify } from './stores/notice.svelte';

/** Songs queued around a double-clicked row of the Songs list. */
export const SONGS_QUEUE = 1000;
/** Of those, how many come before the row (so Previous keeps working). */
export const SONGS_BEFORE = 100;

function failed(err: unknown) {
  notify(t('flow.failed', { message: String(err) }));
}

/** Plays `ids` from `start`, reporting a failure above the ring. */
export async function playIds(ids: string[], start: number): Promise<void> {
  if (ids.length === 0) return;
  try {
    await playerPlay(ids, Math.max(0, Math.min(start, ids.length - 1)));
  } catch (err) {
    failed(err);
  }
}

/** Plays an album from its first track, from `trackId`, or shuffled. */
export async function playAlbum(albumId: string, options: { trackId?: string; shuffled?: boolean } = {}) {
  try {
    const ids = (await libraryAlbumTracks(albumId)).map((track) => track.id);
    if (options.shuffled) return playIds(shuffle(ids), 0);
    return playIds(ids, options.trackId ? Math.max(0, ids.indexOf(options.trackId)) : 0);
  } catch (err) {
    failed(err);
  }
}

/** Plays the Songs list from row `index`: queues the rows around it in list order. */
export async function playSongsFrom(index: number): Promise<void> {
  const offset = Math.max(0, index - SONGS_BEFORE);
  try {
    const ids = (await libraryTracksRange(offset, SONGS_QUEUE)).map((track) => track.id);
    return playIds(ids, index - offset);
  } catch (err) {
    failed(err);
  }
}
