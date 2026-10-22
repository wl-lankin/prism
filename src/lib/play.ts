import { t } from './i18n';
import {
  libraryAlbumTracks,
  libraryTracksRange,
  navidromePlaylistEntries,
  playerPlay,
  playlistEntries,
} from './ipc';
import { notify } from './stores/notice.svelte';
import { updateSettings } from './stores/settings.svelte';
import type { PlaylistEntry, Source } from './types';

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

/** Plays playlist entries from the one at `fromPosition` (the first one by default). Entries
 *  whose track is not in the index are skipped; when the chosen entry is one of them, playback
 *  starts at the next playable entry, and with none after it nothing plays. `shuffled` plays
 *  every playable entry in random order. `remote` words the notice for a Navidrome playlist
 *  with nothing playable. */
export async function playEntries(
  list: PlaylistEntry[],
  fromPosition = 0,
  options: { shuffled?: boolean; remote?: boolean } = {},
): Promise<void> {
  const ids: string[] = [];
  let start = -1;
  for (const entry of list) {
    if (!entry.track) continue;
    if (start < 0 && entry.position >= fromPosition) start = ids.length;
    ids.push(entry.trackId);
  }
  if (!ids.length) {
    notify(t(options.remote ? 'playlist.remoteNothingPlayable' : 'playlist.nothingPlayable'));
    return;
  }
  if (options.shuffled) return playShuffled(ids);
  if (start < 0) return;
  return playIds(ids, start);
}

/** Plays a Prism playlist from the entry at `fromPosition`, as `playEntries` does. */
export async function playPlaylist(
  id: number,
  fromPosition = 0,
  options: { shuffled?: boolean } = {},
): Promise<void> {
  try {
    await playEntries(await playlistEntries(id), fromPosition, options);
  } catch (err) {
    failed(err);
  }
}

/** Plays a Navidrome playlist (`navidrome:<id>`) from its first available entry. */
export async function playRemotePlaylist(id: string, options: { shuffled?: boolean } = {}): Promise<void> {
  try {
    await playEntries(await navidromePlaylistEntries(id), 0, { ...options, remote: true });
  } catch (err) {
    failed(err);
  }
}

/** Plays the Songs list (of one source, or of every source) from row `index`: queues the rows
 *  around it in list order. */
export async function playSongsFrom(index: number, source?: Source): Promise<void> {
  const offset = Math.max(0, index - SONGS_BEFORE);
  try {
    const ids = (await libraryTracksRange(offset, SONGS_QUEUE, source)).map((track) => track.id);
    return playIds(ids, index - offset);
  } catch (err) {
    failed(err);
  }
}
