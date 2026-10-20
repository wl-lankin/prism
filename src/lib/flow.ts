import { libraryAlbumTracks, libraryAlbums, playerPlay } from './ipc';

/** The most tracks an idle ring click queues. */
export const FLOW_CAP = 2000;

/** Fisher-Yates, in place. */
export function shuffle<T>(items: T[]): T[] {
  for (let i = items.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [items[i], items[j]] = [items[j], items[i]];
  }
  return items;
}

/**
 * The M1 stand-in for Flow: queues the whole library shuffled (at most `FLOW_CAP` tracks)
 * and starts playing. Albums are visited in random order and fetching stops once the cap is
 * reached, so a large library does not cost one call per album. Resolves 'empty' when the
 * library has no tracks. Rejects with the backend's message when playback cannot start.
 */
export async function playAllShuffled(): Promise<'played' | 'empty'> {
  const albums = shuffle(await libraryAlbums());
  const ids: string[] = [];
  for (const album of albums) {
    if (ids.length >= FLOW_CAP) break;
    for (const track of await libraryAlbumTracks(album.id)) ids.push(track.id);
  }
  if (ids.length === 0) return 'empty';
  await playerPlay(shuffle(ids).slice(0, FLOW_CAP), 0);
  return 'played';
}
