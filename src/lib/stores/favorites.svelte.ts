import { SvelteSet } from 'svelte/reactivity';
import { t } from '../i18n';
import { favoritesIds, favoritesSet, onFavoritesChanged } from '../ipc';
import type { FavKind, FavoriteIds } from '../types';
import { navidrome, navidromeErrorText, navidromeStatusKnown } from './navidrome.svelte';
import { notify } from './notice.svelte';

/** Every favorite id, by kind. Track, album and artist ids (a local artist's id is its name,
 *  a Navidrome artist's `navidrome:<id>`). The sets are filled from `favorites_ids` and
 *  changed in place, so `has()` in a template follows each id. */
class FavoritesStore {
  tracks = new SvelteSet<string>();
  albums = new SvelteSet<string>();
  artists = new SvelteSet<string>();
  /** Station ids (`radio:<uuid>`). */
  stations = new SvelteSet<string>();
  /** Bumped whenever the ids were reloaded, so views showing favorite lists refetch them. */
  version = $state(0);
}

export const favorites = new FavoritesStore();

function setOf(kind: FavKind): SvelteSet<string> {
  if (kind === 'track') return favorites.tracks;
  if (kind === 'album') return favorites.albums;
  return kind === 'station' ? favorites.stations : favorites.artists;
}

function replace(set: SvelteSet<string>, ids: string[]) {
  const next = new Set(ids);
  for (const id of set) if (!next.has(id)) set.delete(id);
  for (const id of next) set.add(id);
}

/** Changes still on their way to the backend: a reload must not undo them meanwhile. */
const pending = new Map<string, boolean>();
const pendingKey = (kind: FavKind, id: string) => `${kind}\u0000${id}`;

function apply(ids: FavoriteIds) {
  replace(favorites.tracks, ids.tracks ?? []);
  replace(favorites.albums, ids.albums ?? []);
  replace(favorites.artists, ids.artists ?? []);
  replace(favorites.stations, ids.stations ?? []);
  for (const [key, on] of pending) {
    const [kind, id] = key.split('\u0000') as [FavKind, string];
    if (on) setOf(kind).add(id);
    else setOf(kind).delete(id);
  }
  favorites.version++;
}

async function reload(): Promise<void> {
  try {
    apply(await favoritesIds());
  } catch (err) {
    console.error('loading the favorites failed', err);
  }
}

export function isFavorite(kind: FavKind, id: string): boolean {
  return setOf(kind).has(id);
}

/** Whether a heart may change `id` now. A Navidrome item needs a signed-in account (online,
 *  offline or failing alike: the backend queues the star); while signed out the backend
 *  refuses it. Before the first status answer every id counts as heartable, so nothing flashes
 *  dimmed at startup; a real refusal then comes back as "not connected" with its notice. */
export function heartable(id: string): boolean {
  return !id.startsWith('navidrome:') || !navidromeStatusKnown.loaded || navidrome.state !== 'off';
}

/** Shows the change at once, then saves it; a failure puts the old state back with a notice. */
export async function setFavorite(kind: FavKind, id: string, on: boolean): Promise<void> {
  const set = setOf(kind);
  const was = set.has(id);
  const key = pendingKey(kind, id);
  pending.set(key, on);
  if (on) set.add(id);
  else set.delete(id);
  try {
    await favoritesSet(kind, id, on);
  } catch (err) {
    // Only undo when no later click has changed the same item since.
    if (pending.get(key) === on) {
      if (was) set.add(id);
      else set.delete(id);
    }
    // A heart on a Navidrome item while signed out says so in the UI language.
    const text = navidromeErrorText(err);
    notify(text === t('navidrome.notConnected') ? text : t('favorites.failed', { message: text }));
  } finally {
    if (pending.get(key) === on) pending.delete(key);
  }
}

/** Flips one favorite. */
export function toggleFavorite(kind: FavKind, id: string): Promise<void> {
  return setFavorite(kind, id, !isFavorite(kind, id));
}

/** The song lists' shortcut for the selected row's heart: "f" without modifiers. */
export const FAVORITE_KEY = 'F';

export function isFavoriteKey(event: KeyboardEvent): boolean {
  return (
    event.key.toLowerCase() === 'f' && !event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey
  );
}

/** Loads the favorite ids, then follows `favorites://changed`. Resolves with the cleanup.
 *  The listener is attached before the first load, so no change between the two is missed. */
export async function initFavorites(): Promise<() => void> {
  let stop: (() => void) | undefined;
  try {
    stop = await onFavoritesChanged(() => void reload());
  } catch (err) {
    console.error('following the favorites failed', err);
  }
  await reload();
  return () => stop?.();
}
