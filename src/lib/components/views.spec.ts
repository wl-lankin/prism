import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { Album, Artist, Track } from '../types';

const track = (n: number, album = 'a1'): Track => ({
  id: `local:t${n}`,
  source: 'local',
  title: `Song ${n}`,
  artist: 'Lumen Field',
  albumArtist: 'Lumen Field',
  album: 'Glass Tides',
  albumId: album,
  trackNo: n,
  discNo: 1,
  durationMs: 180_000,
  year: 2021,
  genre: null,
  format: 'flac',
  sampleRate: 44_100,
  bitDepth: 16,
  bitrateKbps: null,
  coverId: null,
  path: `C:/demo/${n}.flac`,
  replayGainTrackDb: null,
  replayGainAlbumDb: null,
  replayGainTrackPeak: null,
  replayGainAlbumPeak: null,
});

const album: Album = {
  id: 'a1',
  source: 'local',
  title: 'Glass Tides',
  artist: 'Lumen Field',
  year: 2021,
  coverId: null,
  trackCount: 3,
  durationMs: 540_000,
};
const artist: Artist = { name: 'Lumen Field', albumCount: 1, trackCount: 3, coverId: null };

const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
  switch (cmd) {
    case 'library_search':
      if ((args as { q: string }).q === 'zz') return { albums: [], artists: [], tracks: [] };
      return { albums: [album], artists: [artist], tracks: [track(2)] };
    case 'library_album_tracks':
      return [track(1), track(2), track(3)];
    case 'favorites_tracks':
      return [track(1), track(2)];
    case 'favorites_albums':
    case 'favorites_artists':
      return [];
    case 'library_tracks_range': {
      const { offset, limit } = args as { offset: number; limit: number };
      return Array.from({ length: limit }, (_, i) => track(offset + i));
    }
    default:
      return undefined;
  }
});
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

// jsdom has no layout: no ResizeObserver (bind:clientHeight) and no scrollIntoView.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= function () {};

const { default: SearchPalette } = await import('./SearchPalette.svelte');
const { default: SongList } = await import('./SongList.svelte');
const { library } = await import('../stores/library.svelte');
const { nav } = await import('../stores/nav.svelte');
const { setLang } = await import('../i18n');

let target: HTMLElement;
beforeEach(() => {
  setLang('en');
  invoke.mockClear();
  target = document.createElement('div');
  document.body.append(target);
});
afterEach(() => {
  target.remove();
  vi.useRealTimers();
});

async function settle() {
  for (let i = 0; i < 5; i++) {
    await tick();
    await Promise.resolve();
  }
  flushSync();
}

/** Page requests wait one animation frame. */
const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));

function type(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
}

function key(el: Element, k: string) {
  el.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true }));
  flushSync();
}

/** Mounts an open palette; `state.open` follows the component's bindable prop. */
function palette() {
  const state = { open: true };
  const app = mount(SearchPalette, {
    target,
    props: {
      get open() {
        return state.open;
      },
      set open(v: boolean) {
        state.open = v;
      },
    },
  });
  flushSync();
  return { app, state, input: target.querySelector('input')! };
}

async function search(input: HTMLInputElement, q: string) {
  type(input, q);
  vi.advanceTimersByTime(120);
  await settle();
}

it('debounces the search by 120 ms and groups the results', async () => {
  vi.useFakeTimers();
  const { app, input } = palette();
  type(input, 'gl');
  type(input, 'glass');
  vi.advanceTimersByTime(119);
  expect(invoke).not.toHaveBeenCalled();
  vi.advanceTimersByTime(1);
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(invoke).toHaveBeenCalledWith('library_search', { q: 'glass', limit: 8 });
  await settle();
  const headings = [...target.querySelectorAll('h2')].map((h) => h.textContent);
  expect(headings).toEqual(['Albums', 'Artists', 'Songs']);
  expect(target.querySelectorAll('[role="option"]')).toHaveLength(3);
  unmount(app);
});

it('opens an album with Enter and closes', async () => {
  vi.useFakeTimers();
  const { app, state, input } = palette();
  await search(input, 'glass');
  key(input, 'Enter');
  expect(nav.view).toBe('album');
  expect(nav.param).toBe('a1');
  expect(state.open).toBe(false);
  unmount(app);
});

it('Enter inside the debounce window searches at once and never acts on the older result', async () => {
  vi.useFakeTimers();
  const { go } = await import('../stores/nav.svelte');
  go('songs');
  const { app, state, input } = palette();
  await search(input, 'glass');
  type(input, 'zz');
  key(input, 'Enter');
  await settle();
  expect(invoke).toHaveBeenLastCalledWith('library_search', { q: 'zz', limit: 8 });
  expect(nav.view).toBe('songs');
  expect(state.open).toBe(true);
  // The pending debounce was cancelled: no second request for the same query.
  vi.advanceTimersByTime(500);
  expect(invoke.mock.calls.filter(([c, a]) => c === 'library_search' && (a as { q: string }).q === 'zz')).toHaveLength(1);
  unmount(app);
});

it('plays a song with its album as the queue', async () => {
  vi.useFakeTimers();
  const { app, input } = palette();
  await search(input, 'glass');
  key(input, 'ArrowDown');
  key(input, 'ArrowDown');
  expect(target.querySelector('[aria-selected="true"]')?.textContent).toContain('Song 2');
  key(input, 'Enter');
  await settle();
  expect(invoke).toHaveBeenCalledWith('library_album_tracks', { albumId: 'a1' });
  expect(invoke).toHaveBeenCalledWith('player_play', {
    trackIds: ['local:t1', 'local:t2', 'local:t3'],
    startIndex: 1,
  });
  unmount(app);
});

it('wraps around with the arrows and closes on Escape', async () => {
  vi.useFakeTimers();
  const { app, state, input } = palette();
  await search(input, 'glass');
  key(input, 'ArrowUp');
  expect(target.querySelector('[aria-selected="true"]')?.textContent).toContain('Song 2');
  key(input, 'Escape');
  expect(state.open).toBe(false);
  unmount(app);
});

it('fetches songs in pages of 200 and renders only the rows near the viewport', async () => {
  library.trackCount = 30_000;
  library.version++;
  const app = mount(SongList, { target });
  await frame();
  await settle();
  const ranges = invoke.mock.calls.filter(([cmd]) => cmd === 'library_tracks_range');
  expect(ranges).toEqual([['library_tracks_range', { offset: 0, limit: 200 }]]);
  const rows = target.querySelectorAll('[role="option"]');
  expect(rows.length).toBeGreaterThan(0);
  expect(rows.length).toBeLessThan(60);
  expect(rows[0].textContent).toContain('Song 0');
  expect(rows[0].getAttribute('aria-posinset')).toBe('1');
  expect(rows[0].getAttribute('aria-setsize')).toBe('30000');
  expect((target.querySelector('[role="listbox"]') as HTMLElement).style.height).toBe(`${30_000 * 44}px`);

  // A library change hides the old rows at once instead of showing stale ones.
  library.version++;
  flushSync();
  expect(target.querySelector('[role="option"]')?.textContent).not.toContain('Song 0');
  await frame();
  await settle();
  expect(target.querySelector('[role="option"]')?.textContent).toContain('Song 0');

  // A double-click plays the list from that row.
  target.querySelectorAll('[role="option"]')[3].dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
  await settle();
  expect(invoke).toHaveBeenCalledWith('library_tracks_range', { offset: 0, limit: 1000 });
  expect(invoke).toHaveBeenCalledWith('player_play', expect.objectContaining({ startIndex: 3 }));
  unmount(app);
});

it('renders a favorite heart as pressed and toggles it on click', async () => {
  const { default: Heart } = await import('./Heart.svelte');
  const { favorites } = await import('../stores/favorites.svelte');
  favorites.tracks.add('local:t1');
  const app = mount(Heart, { target, props: { kind: 'track', id: 'local:t1', label: 'Favorite Song 1' } });
  flushSync();
  const button = target.querySelector('button')!;
  expect(button.getAttribute('aria-pressed')).toBe('true');
  expect(button.getAttribute('aria-label')).toBe('Favorite Song 1');

  button.click();
  flushSync();
  expect(button.getAttribute('aria-pressed')).toBe('false');
  expect(invoke).toHaveBeenCalledWith('favorites_set', { kind: 'track', id: 'local:t1', on: false });

  button.click();
  flushSync();
  expect(button.getAttribute('aria-pressed')).toBe('true');
  expect(invoke).toHaveBeenLastCalledWith('favorites_set', { kind: 'track', id: 'local:t1', on: true });
  unmount(app);
});

it('toggles the favorite of the selected Songs row with "f"', async () => {
  const { favorites } = await import('../stores/favorites.svelte');
  favorites.tracks.clear();
  library.trackCount = 50;
  library.version++;
  const app = mount(SongList, { target });
  await frame();
  await settle();
  const list = target.querySelector('[role="listbox"]')!;
  expect(list.getAttribute('aria-keyshortcuts')).toBe('F');
  target.querySelectorAll('[role="option"]')[2].dispatchEvent(new MouseEvent('click', { bubbles: true }));
  flushSync();
  key(list, 'f');
  expect(invoke).toHaveBeenCalledWith('favorites_set', { kind: 'track', id: 'local:t2', on: true });
  expect(favorites.tracks.has('local:t2')).toBe(true);
  // With a modifier it is not the shortcut.
  invoke.mockClear();
  list.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true }));
  flushSync();
  expect(invoke).not.toHaveBeenCalledWith('favorites_set', expect.anything());
  key(list, 'f');
  expect(invoke).toHaveBeenCalledWith('favorites_set', { kind: 'track', id: 'local:t2', on: false });
  unmount(app);
});

it('toggles the favorite of the selected album track with "f"', async () => {
  const { default: AlbumView } = await import('./AlbumView.svelte');
  const { favorites } = await import('../stores/favorites.svelte');
  favorites.tracks.clear();
  const app = mount(AlbumView, { target, props: { albumId: 'a1' } });
  await settle();
  const list = target.querySelector('[role="listbox"]')!;
  key(list, 'ArrowDown');
  key(list, 'f');
  expect(invoke).toHaveBeenCalledWith('favorites_set', { kind: 'track', id: 'local:t1', on: true });
  unmount(app);
});

it('hides a row heart that is not set from pointer and screen reader until the row is selected', async () => {
  const { default: AlbumView } = await import('./AlbumView.svelte');
  const { favorites } = await import('../stores/favorites.svelte');
  favorites.tracks.clear();
  favorites.tracks.add('local:t3');
  const app = mount(AlbumView, { target, props: { albumId: 'a1' } });
  await settle();
  const hearts = [...target.querySelectorAll<HTMLElement>('.tracks .heart')];
  expect(hearts[0].getAttribute('aria-hidden')).toBe('true');
  expect(hearts[0].classList.contains('hidden')).toBe(true);
  expect(hearts[2].getAttribute('aria-hidden')).toBeNull();
  key(target.querySelector('[role="listbox"]')!, 'ArrowDown');
  expect(hearts[0].getAttribute('aria-hidden')).toBeNull();
  expect(hearts[0].classList.contains('hidden')).toBe(false);
  // A focused heart never stays invisible, even when it is not set.
  hearts[1].dispatchEvent(new FocusEvent('focus'));
  flushSync();
  expect(hearts[1].classList.contains('hidden')).toBe(false);
  unmount(app);
});

it('un-hearts the selected Favorites song with "f" and keeps the selection in range', async () => {
  const { default: FavoritesView } = await import('./FavoritesView.svelte');
  const { favorites } = await import('../stores/favorites.svelte');
  favorites.tracks.clear();
  favorites.tracks.add('local:t1');
  favorites.tracks.add('local:t2');
  const app = mount(FavoritesView, { target });
  await settle();
  const list = target.querySelector('[role="listbox"]')!;
  expect(list.querySelectorAll('[role="option"]')).toHaveLength(2);
  key(list, 'End');
  expect(list.getAttribute('aria-activedescendant')).toBe('fav-song-1');
  key(list, 'f');
  expect(invoke).toHaveBeenCalledWith('favorites_set', { kind: 'track', id: 'local:t2', on: false });
  await settle();
  const rows = list.querySelectorAll('[role="option"]');
  expect(rows).toHaveLength(1);
  expect(list.getAttribute('aria-activedescendant')).toBe('fav-song-0');
  expect(rows[0].getAttribute('aria-selected')).toBe('true');
  unmount(app);
});

it('shows no transport heart for a dropped file, and one for a library track', async () => {
  const { default: TransportStrip } = await import('./TransportStrip.svelte');
  const { player } = await import('../stores/player.svelte');
  player.status = 'playing';
  player.track = { ...track(1), id: 'file:C:/drop/a.flac', source: 'local' };
  const app = mount(TransportStrip, { target });
  flushSync();
  expect(target.querySelector('.heart')).toBeNull();
  player.track = track(1);
  flushSync();
  expect(target.querySelector('.heart')?.getAttribute('aria-pressed')).not.toBeNull();
  player.status = 'idle';
  player.track = null;
  flushSync();
  unmount(app);
});

it('renders a heart that is not a favorite as not pressed', async () => {
  const { default: Heart } = await import('./Heart.svelte');
  const app = mount(Heart, { target, props: { kind: 'album', id: 'nope' } });
  flushSync();
  const button = target.querySelector('button')!;
  expect(button.getAttribute('aria-pressed')).toBe('false');
  expect(button.getAttribute('aria-label')).toBe('Favorite');
  unmount(app);
});
