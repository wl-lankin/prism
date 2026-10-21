import { t } from './i18n';
import { libraryAlbumTracks, libraryTracksRange, playerPlay, playlistEntries } from './ipc';
import { notify } from './stores/notice.svelte';
import { updateSettings } from './stores/settings.svelte';

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

/** The Shuffle buttons: turns the shuffle mode on (as the transport's toggle does), then
 *  plays `ids` from a random track. The engine deals the rest in random order and keeps
 *  `ids` as the original order, so turning shuffle off later plays on in list order. */
export async function playShuffled(ids: string[]): Promise<void> {
  if (ids.length === 0) return;
  try {
    await updateSettings({ shuffle: true });
  } catch (err) {
    failed(err);
    return;
  }
  await playIds(ids, Math.floor(Math.random() * ids.length));
}

/** Plays an album from its first track, from `trackId`, or shuffled. */
export async function playAlbum(albumId: string, options: { trackId?: string; shuffled?: boolean } = {}) {
  try {
    const ids = (await libraryAlbumTracks(albumId)).map((track) => track.id);
    if (options.shuffled) return playShuffled(ids);
    return playIds(ids, options.trackId ? Math.max(0, ids.indexOf(options.trackId)) : 0);
  } catch (err) {
    failed(err);
  }
}

/** Plays a playlist from the entry at `fromPosition` (the first one by default). Entries whose
 *  track is not in the index are skipped; when the chosen entry is one of them, playback
 *  starts at the next playable entry, and with none after it nothing plays. `shuffled` plays
 *  every playable entry in random order. */
export async function playPlaylist(
  id: number,
  fromPosition = 0,
  options: { shuffled?: boolean } = {},
): Promise<void> {
  try {
    const ids: string[] = [];
    let start = -1;
    for (const entry of await playlistEntries(id)) {
      if (!entry.track) continue;
      if (start < 0 && entry.position >= fromPosition) start = ids.length;
      ids.push(entry.trackId);
    }
    if (!ids.length) {
      notify(t('playlist.nothingPlayable'));
      return;
    }
    if (options.shuffled) return playShuffled(ids);
    if (start < 0) return;
    return playIds(ids, start);
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
