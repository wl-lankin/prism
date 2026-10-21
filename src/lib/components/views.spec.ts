import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Album, Artist, NavidromeStatus, Track } from '../types';

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
const artist: Artist = {
  id: 'Lumen Field',
  source: 'local',
  name: 'Lumen Field',
  albumCount: 1,
  trackCount: 3,
  coverId: null,
};

/** Track counts of a library with only local tracks. */
const counts = (all: number) => ({ all, local: all, navidrome: 0 });

/** How long the queue is that `player_queue_range` serves. */
let queueLength = 0;

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
    case 'playlists_list':
      return [
        { id: 1, name: 'Road trip', trackCount: 2, durationMs: 360_000, coverIds: [], updatedAt: 2 },
        { id: 2, name: 'Late nights', trackCount: 0, durationMs: 0, coverIds: [], updatedAt: 1 },
      ];
    case 'playlist_create':
      return { id: 7, name: (args as { name: string }).name, trackCount: 0, durationMs: 0, coverIds: [], updatedAt: 3 };
    case 'playlist_entries':
      return [
        { position: 0, trackId: 'local:t1', track: track(1) },
        { position: 1, trackId: 'local:gone', track: null },
        { position: 2, trackId: 'local:t3', track: track(3) },
      ];
    case 'player_queue_range': {
      const { offset, limit } = args as { offset: number; limit: number };
      const end = Math.min(offset + limit, queueLength);
      return Array.from({ length: Math.max(0, end - offset) }, (_, i) => ({
        index: offset + i,
        track: track(offset + i),
      }));
    }
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
// Nor the Web Animations API, which `animate:flip` looks at when rows move.
Element.prototype.getAnimations ??= () => [];

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
  library.trackCounts = counts(30_000);
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

it('asks for a failed song page again while it is in view', async () => {
  const impl = invoke.getMockImplementation()!;
  let failures = 1;
  invoke.mockImplementation(async (cmd, args) => {
    if (cmd === 'library_tracks_range' && failures-- > 0) throw new Error('busy');
    return impl(cmd, args);
  });
  try {
    library.trackCounts = counts(50);
    library.version++;
    const app = mount(SongList, { target });
    await frame();
    await settle();
    expect(target.querySelector('.row .title')).toBeNull();
    await vi.waitFor(
      async () => {
        await settle();
        expect(target.querySelector('.row .title')?.textContent).toBe('Song 0');
      },
      { timeout: 3000, interval: 100 },
    );
    expect(invoke.mock.calls.filter(([cmd]) => cmd === 'library_tracks_range')).toHaveLength(2);
    unmount(app);
  } finally {
    invoke.mockImplementation(impl);
  }
});

it('stops asking for a failing song page after three attempts', async () => {
  const impl = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (cmd, args) => {
    if (cmd === 'library_tracks_range') throw new Error('busy');
    return impl(cmd, args);
  });
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  try {
    library.trackCounts = counts(50);
    library.version++;
    const app = mount(SongList, { target });
    await frame();
    await settle();
    for (let i = 0; i < 5; i++) {
      vi.advanceTimersByTime(1500);
      await settle();
      await frame();
      await settle();
    }
    expect(invoke.mock.calls.filter(([cmd]) => cmd === 'library_tracks_range')).toHaveLength(3);
    unmount(app);
  } finally {
    vi.useRealTimers();
    invoke.mockImplementation(impl);
  }
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
  library.trackCounts = counts(50);
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

describe('Track menu', () => {
  async function trackMenu(props: Record<string, unknown> = {}) {
    const { default: TrackMenu } = await import('./TrackMenu.svelte');
    const { reloadPlaylists } = await import('../stores/playlists.svelte');
    await reloadPlaylists();
    const onclose = vi.fn();
    const app = mount(TrackMenu, {
      target,
      props: { trackIds: ['local:t1', 'local:t2'], x: 40, y: 60, onclose, ...props },
    });
    flushSync();
    return { app, onclose };
  }

  const item = (label: string) =>
    [...target.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
      (el) => el.querySelector('.label')?.textContent?.trim() === label,
    );

  it('lists the playlists and adds the given ids to the chosen one', async () => {
    const { app, onclose } = await trackMenu();
    item('Add to playlist')!.click();
    flushSync();
    expect(item('Road trip')).toBeTruthy();
    expect(item('Late nights')).toBeTruthy();
    expect(item('New playlist...')).toBeTruthy();
    item('Late nights')!.click();
    await settle();
    expect(invoke).toHaveBeenCalledWith('playlist_add', { id: 2, trackIds: ['local:t1', 'local:t2'] });
    expect(onclose).toHaveBeenCalled();
    unmount(app);
  });

  it('creates a playlist with "New playlist..." and then adds the ids to it', async () => {
    const { app, onclose } = await trackMenu();
    item('Add to playlist')!.click();
    flushSync();
    item('New playlist...')!.click();
    flushSync();
    const input = target.querySelector<HTMLInputElement>('.menu input')!;
    expect(input.value).toBe('New playlist');
    input.value = 'Sunday';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    key(input, 'Enter');
    await settle();
    const calls = invoke.mock.calls.map(([cmd]) => cmd);
    expect(invoke).toHaveBeenCalledWith('playlist_create', { name: 'Sunday' });
    expect(invoke).toHaveBeenCalledWith('playlist_add', { id: 7, trackIds: ['local:t1', 'local:t2'] });
    expect(calls.indexOf('playlist_create')).toBeLessThan(calls.indexOf('playlist_add'));
    expect(onclose).toHaveBeenCalled();
    unmount(app);
  });

  it('closes when Tab leaves the new playlist name field', async () => {
    const { app, onclose } = await trackMenu();
    item('Add to playlist')!.click();
    flushSync();
    item('New playlist...')!.click();
    flushSync();
    const input = target.querySelector<HTMLInputElement>('.menu input')!;
    key(input, 'Tab');
    expect(onclose).toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalledWith('playlist_create', expect.anything());
    unmount(app);
  });

  it('offers the favorite, album and artist only where they apply', async () => {
    const { favorites } = await import('../stores/favorites.svelte');
    favorites.tracks.clear();
    const { app, onclose } = await trackMenu({ trackIds: ['local:t1'], albumId: 'a1', artist: 'Lumen Field' });
    expect(item('Add to favorites')).toBeTruthy();
    expect(item('Go to album')).toBeTruthy();
    expect(item('Go to artist')).toBeTruthy();
    item('Go to album')!.click();
    flushSync();
    expect(nav.view).toBe('album');
    expect(nav.param).toBe('a1');
    expect(onclose).toHaveBeenCalled();
    unmount(app);

    const many = await trackMenu();
    expect(item('Add to favorites')).toBeUndefined();
    expect(item('Go to album')).toBeUndefined();
    key(target.querySelector('[role="menu"]')!, 'Escape');
    expect(many.onclose).toHaveBeenCalled();
    unmount(many.app);
  });

  it('opens from a right-click on a song row', async () => {
    library.trackCounts = counts(50);
    library.version++;
    const app = mount(SongList, { target });
    await frame();
    await settle();
    const row = target.querySelectorAll('[role="option"]')[3];
    row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 50, clientY: 80 }));
    await settle();
    expect(row.getAttribute('aria-selected')).toBe('true');
    expect(target.querySelector('[role="menu"]')).not.toBeNull();
    expect(item('Add to favorites')).toBeTruthy();
    unmount(app);
  });
});

describe('Playlist view', () => {
  async function playlistView() {
    const { default: PlaylistView } = await import('./PlaylistView.svelte');
    const { reloadPlaylists } = await import('../stores/playlists.svelte');
    await reloadPlaylists();
    const app = mount(PlaylistView, { target, props: { playlistId: 1 } });
    await settle();
    return { app, list: target.querySelector('[role="listbox"]')! };
  }

  it('dims entries that are not in the library', async () => {
    const { app } = await playlistView();
    const rows = target.querySelectorAll('[role="option"]');
    expect(rows).toHaveLength(3);
    expect(rows[1].classList.contains('missing')).toBe(true);
    expect(rows[0].classList.contains('missing')).toBe(false);
    unmount(app);
  });

  it('renames with Enter and cancels with Escape', async () => {
    const { app } = await playlistView();
    target.querySelector<HTMLElement>('.rename')!.click();
    flushSync();
    let input = target.querySelector<HTMLInputElement>('input.title-input')!;
    expect(input.value).toBe('Road trip');
    input.value = 'Coast road';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    key(input, 'Enter');
    await settle();
    expect(invoke).toHaveBeenCalledWith('playlist_rename', { id: 1, name: 'Coast road' });
    expect(target.querySelector('h1')?.textContent).toContain('Coast road');

    invoke.mockClear();
    target.querySelector<HTMLElement>('.rename')!.click();
    flushSync();
    input = target.querySelector<HTMLInputElement>('input.title-input')!;
    input.value = 'Never mind';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    key(input, 'Escape');
    await settle();
    expect(invoke).not.toHaveBeenCalledWith('playlist_rename', expect.anything());
    expect(target.querySelector('input.title-input')).toBeNull();
    unmount(app);
  });

  it('moves the selected entry with Alt+arrows and removes it with Delete', async () => {
    const { app, list } = await playlistView();
    key(list, 'ArrowDown');
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true }));
    flushSync();
    expect(list.getAttribute('aria-activedescendant')).toBe('pl-row-1');
    key(list, 'Delete');
    await settle();
    expect(invoke).toHaveBeenCalledWith('playlist_move', { id: 1, from: 0, to: 1 });
    expect(invoke).toHaveBeenCalledWith('playlist_remove', { id: 1, positions: [1] });
    unmount(app);
  });

  it('sends quick moves one after the other and ignores a refetch that lands in between', async () => {
    const { playlists } = await import('../stores/playlists.svelte');
    const impl = invoke.getMockImplementation()!;
    const held: (() => void)[] = [];
    invoke.mockImplementation((cmd, args) => {
      if (cmd === 'playlist_move') return new Promise((resolve) => held.push(() => resolve(undefined)));
      return impl(cmd, args);
    });
    try {
      const { app, list } = await playlistView();
      const titles = () => [...list.querySelectorAll('[role="option"] .title')].map((el) => el.textContent);
      const altDown = () =>
        list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true }));
      key(list, 'ArrowDown');
      altDown();
      await settle();
      const moves = () => invoke.mock.calls.filter(([cmd]) => cmd === 'playlist_move');
      expect(moves()).toEqual([['playlist_move', { id: 1, from: 0, to: 1 }]]);

      // The backend announces a change while the first move is still on its way: the list it
      // serves (the old order) must not replace the order shown.
      playlists.version++;
      await settle();
      expect(titles()).toEqual(['Not in your library', 'Song 1', 'Song 3']);

      altDown();
      await settle();
      expect(titles()).toEqual(['Not in your library', 'Song 3', 'Song 1']);
      // The second move waits for the first, and is computed against the list after it.
      expect(moves()).toHaveLength(1);
      held.shift()!();
      await settle();
      expect(moves()).toEqual([
        ['playlist_move', { id: 1, from: 0, to: 1 }],
        ['playlist_move', { id: 1, from: 1, to: 2 }],
      ]);
      expect(titles()).toEqual(['Not in your library', 'Song 3', 'Song 1']);

      // Once the queue drains, the entries are fetched once more.
      const fetches = invoke.mock.calls.filter(([cmd]) => cmd === 'playlist_entries').length;
      held.shift()!();
      await settle();
      expect(invoke.mock.calls.filter(([cmd]) => cmd === 'playlist_entries')).toHaveLength(fetches + 1);
      unmount(app);
    } finally {
      invoke.mockImplementation(impl);
    }
  });

  it('plays from a double-clicked row, skipping missing entries', async () => {
    const { app } = await playlistView();
    target.querySelectorAll('[role="option"]')[2].dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    await settle();
    expect(invoke).toHaveBeenCalledWith('player_play', { trackIds: ['local:t1', 'local:t3'], startIndex: 1 });
    unmount(app);
  });

  it('does nothing on a double-click or Enter on a missing row', async () => {
    const { app, list } = await playlistView();
    invoke.mockClear();
    const missing = target.querySelectorAll('[role="option"]')[1];
    missing.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    missing.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    flushSync();
    key(list, 'Enter');
    await settle();
    expect(invoke).not.toHaveBeenCalledWith('playlist_entries', expect.anything());
    expect(invoke).not.toHaveBeenCalledWith('player_play', expect.anything());
    unmount(app);
  });
});

describe('Now Playing', () => {
  async function nowPlaying(index: number, length: number) {
    const { default: NowPlaying } = await import('./NowPlaying.svelte');
    const { player } = await import('../stores/player.svelte');
    queueLength = length;
    Object.assign(player, { status: 'playing', track: track(index), queueIndex: index, queueLength: length });
    player.queueVersion++;
    const app = mount(NowPlaying, { target });
    await frame();
    await settle();
    return { app, player };
  }

  const queueCalls = () => invoke.mock.calls.filter(([cmd]) => cmd === 'player_queue_range');

  afterEach(async () => {
    const { player } = await import('../stores/player.svelte');
    Object.assign(player, { status: 'idle', track: null, queueIndex: 0, queueLength: 0, flow: undefined });
  });

  it('requests the page that holds the current entry and highlights it', async () => {
    const { app } = await nowPlaying(450, 1000);
    expect(queueCalls()).toContainEqual(['player_queue_range', { offset: 400, limit: 200 }]);
    const current = target.querySelector('[aria-current="true"]');
    expect(current?.getAttribute('data-index')).toBe('450');
    expect(current?.textContent).toContain('Song 450');
    expect(target.querySelectorAll('.qrow').length).toBeLessThan(60);
    unmount(app);
  });

  it('jumps to an entry when its row is clicked', async () => {
    const { app } = await nowPlaying(0, 20);
    const row = target.querySelector<HTMLElement>('[data-index="5"]')!;
    row.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await settle();
    expect(invoke).toHaveBeenCalledWith('player_jump', { index: 5 });
    unmount(app);
  });

  it('refetches the queue when its version changes', async () => {
    const { app, player } = await nowPlaying(0, 20);
    expect(queueCalls()).toHaveLength(1);
    player.queueVersion++;
    await frame();
    await settle();
    expect(queueCalls()).toHaveLength(2);
    unmount(app);
  });

  it('shows the Flow variant in the header only in Flow', async () => {
    const { app, player } = await nowPlaying(0, 20);
    expect(target.textContent).not.toContain('Flow ·');
    (player as { flow?: unknown }).flow = { kind: 'genre', genre: 'Ambient' };
    flushSync();
    expect(target.textContent).toContain('Flow · Ambient');
    (player as { flow?: unknown }).flow = { kind: 'favorites' };
    flushSync();
    expect(target.textContent).toContain('Flow · Favorites');
    unmount(app);
  });
});

describe('Row play button', () => {
  afterEach(async () => {
    const { player } = await import('../stores/player.svelte');
    Object.assign(player, { status: 'idle', track: null, queueIndex: 0, queueLength: 0 });
  });

  const rowAt = (i: number) => target.querySelector<HTMLElement>(`[role="option"][data-index="${i}"]`)!;
  const playIn = (row: HTMLElement) => row.querySelector<HTMLButtonElement>('.rowplay .play')!;

  function hover(row: HTMLElement, on = true) {
    row.dispatchEvent(new MouseEvent(on ? 'mouseenter' : 'mouseleave'));
    flushSync();
  }

  async function click(button: HTMLElement) {
    button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await settle();
  }

  it('turns the number into a play button on hover or selection, out of the tab order', async () => {
    const { default: AlbumView } = await import('./AlbumView.svelte');
    const app = mount(AlbumView, { target, props: { albumId: 'a1' } });
    await settle();
    const row = rowAt(1);
    const button = playIn(row);
    expect(button.tabIndex).toBe(-1);
    expect(button.getAttribute('aria-label')).toBe('Play Song 2');
    expect(button.getAttribute('aria-hidden')).toBe('true');
    expect(row.querySelector('.rowplay')!.classList.contains('shown')).toBe(false);
    expect(row.querySelector('.mark')!.textContent?.trim()).toBe('2');

    hover(row);
    expect(row.querySelector('.rowplay')!.classList.contains('shown')).toBe(true);
    expect(button.getAttribute('aria-hidden')).toBeNull();
    hover(row, false);
    expect(row.querySelector('.rowplay')!.classList.contains('shown')).toBe(false);

    // Keyboard selection reveals it too.
    const list = target.querySelector('[role="listbox"]')!;
    key(list, 'ArrowDown');
    key(list, 'ArrowDown');
    expect(row.querySelector('.rowplay')!.classList.contains('shown')).toBe(true);
    unmount(app);
  });

  it('plays the album from the clicked row, like a double-click', async () => {
    const { default: AlbumView } = await import('./AlbumView.svelte');
    const app = mount(AlbumView, { target, props: { albumId: 'a1' } });
    await settle();
    invoke.mockClear();
    const row = rowAt(2);
    hover(row);
    await click(playIn(row));
    expect(invoke).toHaveBeenCalledWith('player_play', {
      trackIds: ['local:t1', 'local:t2', 'local:t3'],
      startIndex: 2,
    });
    // The click is the button's: it does not select the row.
    expect(row.getAttribute('aria-selected')).toBe('false');
    unmount(app);
  });

  it('leaves focus on the list when the row play button or heart is clicked', async () => {
    const { default: AlbumView } = await import('./AlbumView.svelte');
    const app = mount(AlbumView, { target, props: { albumId: 'a1' } });
    await settle();
    const list = target.querySelector<HTMLElement>('[role="listbox"]')!;
    list.focus();
    const row = rowAt(1);
    hover(row);
    for (const button of [playIn(row), row.querySelector<HTMLElement>('.heart')!]) {
      const down = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
      button.dispatchEvent(down);
      // A cancelled mousedown is what keeps the browser from focusing the button.
      expect(down.defaultPrevented).toBe(true);
      await click(button);
      expect(document.activeElement).toBe(list);
      expect(document.activeElement).not.toBe(button);
    }
    unmount(app);
  });

  it('hides the playlist row play button while a row is dragged', async () => {
    const { default: PlaylistView } = await import('./PlaylistView.svelte');
    const { reloadPlaylists } = await import('../stores/playlists.svelte');
    await reloadPlaylists();
    const app = mount(PlaylistView, { target, props: { playlistId: 1 } });
    await settle();
    const list = target.querySelector<HTMLElement>('[role="listbox"]')!;
    const row = rowAt(2);
    hover(row);
    expect(row.querySelector('.rowplay')!.classList.contains('shown')).toBe(true);
    const at = { bubbles: true, button: 0, pointerId: 1, clientY: 100 };
    row.querySelector('.name')!.dispatchEvent(new PointerEvent('pointerdown', at));
    list.dispatchEvent(new PointerEvent('pointermove', { ...at, clientY: 140 }));
    flushSync();
    expect(list.classList.contains('dragging')).toBe(true);
    expect(row.querySelector('.rowplay')!.classList.contains('shown')).toBe(false);
    list.dispatchEvent(new PointerEvent('pointercancel', at));
    flushSync();
    expect(row.querySelector('.rowplay')!.classList.contains('shown')).toBe(true);
    unmount(app);
  });

  it('plays the Songs list around the clicked row', async () => {
    library.trackCounts = counts(500);
    library.version++;
    const app = mount(SongList, { target });
    await frame();
    await settle();
    invoke.mockClear();
    await click(playIn(rowAt(4)));
    expect(invoke).toHaveBeenCalledWith('library_tracks_range', { offset: 0, limit: 1000 });
    const play = invoke.mock.calls.find(([cmd]) => cmd === 'player_play');
    expect(play?.[1]).toMatchObject({ startIndex: 4 });
    expect((play?.[1] as { trackIds: string[] }).trackIds[4]).toBe('local:t4');
    unmount(app);
  });

  it('plays the favorite songs from the clicked row', async () => {
    const { default: FavoritesView } = await import('./FavoritesView.svelte');
    const { favorites } = await import('../stores/favorites.svelte');
    favorites.tracks.clear();
    favorites.tracks.add('local:t1');
    favorites.tracks.add('local:t2');
    const app = mount(FavoritesView, { target });
    await settle();
    invoke.mockClear();
    await click(playIn(rowAt(1)));
    expect(invoke).toHaveBeenCalledWith('player_play', { trackIds: ['local:t1', 'local:t2'], startIndex: 1 });
    unmount(app);
  });

  it('plays the playlist from the clicked row and has no button on a missing entry', async () => {
    const { default: PlaylistView } = await import('./PlaylistView.svelte');
    const { reloadPlaylists } = await import('../stores/playlists.svelte');
    await reloadPlaylists();
    const app = mount(PlaylistView, { target, props: { playlistId: 1 } });
    await settle();
    expect(rowAt(1).querySelector('.rowplay')).toBeNull();
    invoke.mockClear();
    await click(playIn(rowAt(2)));
    expect(invoke).toHaveBeenCalledWith('player_play', { trackIds: ['local:t1', 'local:t3'], startIndex: 1 });
    unmount(app);
  });

  it('jumps to the clicked queue entry in Now Playing', async () => {
    const { default: NowPlaying } = await import('./NowPlaying.svelte');
    const { player } = await import('../stores/player.svelte');
    queueLength = 20;
    Object.assign(player, { status: 'playing', track: track(0), queueIndex: 0, queueLength: 20 });
    player.queueVersion++;
    const app = mount(NowPlaying, { target });
    await frame();
    await settle();
    invoke.mockClear();
    await click(playIn(rowAt(6)));
    expect(invoke).toHaveBeenCalledWith('player_jump', { index: 6 });
    unmount(app);
  });

  it('keeps the equalizer on the playing row and toggles pause and play from it on hover', async () => {
    const { default: AlbumView } = await import('./AlbumView.svelte');
    const { player } = await import('../stores/player.svelte');
    Object.assign(player, { status: 'playing', track: track(2) });
    const app = mount(AlbumView, { target, props: { albumId: 'a1' } });
    await settle();
    const row = rowAt(1);
    expect(row.querySelector('.mark [role="img"]')?.getAttribute('aria-label')).toBe('Now playing');
    // Selecting the playing row keeps the marker; only the pointer swaps it.
    const list = target.querySelector('[role="listbox"]')!;
    key(list, 'ArrowDown');
    key(list, 'ArrowDown');
    expect(row.querySelector('.rowplay')!.classList.contains('shown')).toBe(false);

    hover(row);
    const button = playIn(row);
    expect(row.querySelector('.rowplay')!.classList.contains('shown')).toBe(true);
    expect(button.getAttribute('aria-label')).toBe('Pause Song 2');
    invoke.mockClear();
    await click(button);
    expect(invoke).toHaveBeenCalledWith('player_toggle', undefined);
    expect(invoke).not.toHaveBeenCalledWith('player_play', expect.anything());

    player.status = 'paused';
    flushSync();
    expect(button.getAttribute('aria-label')).toBe('Resume Song 2');
    invoke.mockClear();
    await click(button);
    expect(invoke).toHaveBeenCalledWith('player_toggle', undefined);
    unmount(app);
  });
});

// Navidrome favorites, artist hearts, signed-out hearts, the offline notice and
// streamed quality labels.
describe('Navidrome favorites and quality', () => {
  const ndArtist: Artist = {
    id: 'navidrome:ar1',
    source: 'navidrome',
    name: 'Lumen Field',
    albumCount: 2,
    trackCount: 9,
    coverId: null,
  };
  const ndTrack = (over: Partial<Track> = {}): Track => ({
    ...track(1),
    id: 'navidrome:s1',
    source: 'navidrome',
    albumId: 'navidrome:al1',
    path: null,
    bitDepth: 24,
    sampleRate: 96_000,
    ...over,
  });

  /** Serves `answers` for the named commands and the shared mock for the rest. */
  function serve(answers: Record<string, unknown>) {
    const base = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (cmd, args) => (cmd in answers ? answers[cmd] : base(cmd, args)));
    return () => invoke.mockImplementation(base);
  }

  let restore: (() => void) | undefined;
  beforeEach(async () => {
    const { navidrome, navidromeStatusKnown } = await import('../stores/navidrome.svelte');
    const { favorites } = await import('../stores/favorites.svelte');
    navidrome.state = 'online';
    navidromeStatusKnown.loaded = true;
    favorites.tracks.clear();
    favorites.albums.clear();
    favorites.artists.clear();
  });
  afterEach(async () => {
    restore?.();
    restore = undefined;
    const { navidrome, navidromeStatusKnown } = await import('../stores/navidrome.svelte');
    const { player } = await import('../stores/player.svelte');
    const { settings } = await import('../stores/settings.svelte');
    const { notice } = await import('../stores/notice.svelte');
    navidrome.state = 'off';
    navidromeStatusKnown.loaded = false;
    settings.navidromeQuality = 'original';
    notice.text = null;
    Object.assign(player, { status: 'idle', track: null });
    library.artists = null;
  });

  async function favoritesView(artists: Artist[]) {
    const { default: FavoritesView } = await import('./FavoritesView.svelte');
    const { favorites } = await import('../stores/favorites.svelte');
    for (const a of artists) favorites.artists.add(a.id);
    restore = serve({ favorites_tracks: [], favorites_albums: [], favorites_artists: artists });
    const app = mount(FavoritesView, { target });
    await settle();
    (target.querySelector('#fav-tab-artist') as HTMLElement).click();
    flushSync();
    return app;
  }

  const chips = () => [...target.querySelectorAll('.chips [role="radio"]')].map((x) => x.textContent?.trim());
  const artistNames = () => [...target.querySelectorAll('.artists .text')].map((x) => x.textContent);

  it('navidrome_chip_appears_with_navidrome_favorites', async () => {
    const app = await favoritesView([ndArtist]);
    expect(chips()).toEqual(['All', 'Local files', 'Navidrome']);
    unmount(app);
  });

  it('artist_favorites_filter_by_source', async () => {
    const other: Artist = { ...artist, id: 'Quiet Harbor', name: 'Quiet Harbor' };
    const app = await favoritesView([other, ndArtist]);
    expect(artistNames()).toEqual(['Quiet Harbor', 'Lumen Field']);

    const navidromeChip = [...target.querySelectorAll<HTMLElement>('.chips [role="radio"]')][2];
    navidromeChip.click();
    flushSync();
    expect(artistNames()).toEqual(['Lumen Field']);
    // The heart of a listed artist is its id, not its name.
    const heart = target.querySelector('.artists .heart') as HTMLElement;
    heart.click();
    flushSync();
    expect(invoke).toHaveBeenCalledWith('favorites_set', { kind: 'artist', id: 'navidrome:ar1', on: false });
    unmount(app);
  });

  it('says so when only another source has favorite artists', async () => {
    const app = await favoritesView([ndArtist]);
    [...target.querySelectorAll<HTMLElement>('.chips [role="radio"]')][1].click();
    flushSync();
    expect(target.querySelector('.empty .lead')?.textContent).toBe('None of these favorites come from Local files.');
    unmount(app);
  });

  async function artistView(artists: Artist[]) {
    const { default: ArtistView } = await import('./ArtistView.svelte');
    library.artists = artists;
    const app = mount(ArtistView, { target, props: { name: 'Lumen Field' } });
    await settle();
    return { app, heart: target.querySelector('.hero .heart') as HTMLButtonElement };
  }

  const setCalls = () => invoke.mock.calls.filter(([cmd]) => cmd === 'favorites_set').map(([, args]) => args);

  it('artist_view_hearts_the_navidrome_id', async () => {
    // Navidrome only: the Navidrome id, so the favorite resolves and reaches the server.
    let view = await artistView([ndArtist]);
    view.heart.click();
    flushSync();
    expect(setCalls()).toEqual([{ kind: 'artist', id: 'navidrome:ar1', on: true }]);
    unmount(view.app);

    // Local only: the name, as in M2.
    const { favorites } = await import('../stores/favorites.svelte');
    favorites.artists.clear();
    invoke.mockClear();
    view = await artistView([artist]);
    view.heart.click();
    flushSync();
    expect(setCalls()).toEqual([{ kind: 'artist', id: 'Lumen Field', on: true }]);
    unmount(view.app);
  });

  it('an artist in both sources hearts both, and un-hearts both from either', async () => {
    const { favorites } = await import('../stores/favorites.svelte');
    const view = await artistView([artist, ndArtist]);
    expect(view.heart.getAttribute('aria-pressed')).toBe('false');
    view.heart.click();
    flushSync();
    expect(setCalls()).toEqual([
      { kind: 'artist', id: 'Lumen Field', on: true },
      { kind: 'artist', id: 'navidrome:ar1', on: true },
    ]);

    // Starred on the server only (imported): the heart is on, and one click clears both
    // without touching the side that is already off.
    favorites.artists.clear();
    favorites.artists.add('navidrome:ar1');
    flushSync();
    expect(view.heart.getAttribute('aria-pressed')).toBe('true');
    invoke.mockClear();
    view.heart.click();
    flushSync();
    expect(setCalls()).toEqual([{ kind: 'artist', id: 'navidrome:ar1', on: false }]);
    expect(view.heart.getAttribute('aria-pressed')).toBe('false');
    unmount(view.app);
  });

  it('mutes hearts on Navidrome items while signed out, and keeps local ones', async () => {
    const { default: Heart } = await import('./Heart.svelte');
    const { navidrome } = await import('../stores/navidrome.svelte');
    const { notice } = await import('../stores/notice.svelte');
    navidrome.state = 'off';
    const app = mount(Heart, { target, props: { kind: 'track', id: 'navidrome:s1' } });
    const local = mount(Heart, { target, props: { kind: 'track', id: 'local:t1' } });
    flushSync();
    const [nd, loc] = [...target.querySelectorAll<HTMLButtonElement>('.heart')];
    expect(nd.getAttribute('aria-disabled')).toBe('true');
    expect(nd.disabled).toBe(false);
    expect(nd.title).toBe('Navidrome is not connected. Sign in again in the settings.');
    expect(loc.getAttribute('aria-disabled')).toBeNull();
    // A click says why instead of changing anything.
    nd.click();
    flushSync();
    expect(invoke).not.toHaveBeenCalledWith('favorites_set', expect.anything());
    expect(nd.getAttribute('aria-pressed')).toBe('false');
    expect(notice.text).toBe('Navidrome is not connected. Sign in again in the settings.');

    // Offline (or any state but off) hearts still work: the backend queues them.
    navidrome.state = 'offline';
    flushSync();
    expect(nd.getAttribute('aria-disabled')).toBeNull();
    nd.click();
    flushSync();
    expect(invoke).toHaveBeenCalledWith('favorites_set', { kind: 'track', id: 'navidrome:s1', on: true });
    unmount(app);
    unmount(local);
  });

  it('does not mute Navidrome hearts before the first status answer', async () => {
    const { default: Heart } = await import('./Heart.svelte');
    const { navidrome, navidromeStatusKnown } = await import('../stores/navidrome.svelte');
    // The store's starting value, before the backend answered: not a sign-out.
    navidrome.state = 'off';
    navidromeStatusKnown.loaded = false;
    const app = mount(Heart, { target, props: { kind: 'track', id: 'navidrome:s1' } });
    flushSync();
    const heart = target.querySelector<HTMLButtonElement>('.heart')!;
    expect(heart.getAttribute('aria-disabled')).toBeNull();
    expect(heart.classList.contains('muted')).toBe(false);
    // Once the answer says signed out, it mutes.
    navidromeStatusKnown.loaded = true;
    flushSync();
    expect(heart.getAttribute('aria-disabled')).toBe('true');
    unmount(app);
  });

  it('the artist heart writes all its ids before the first status answer', async () => {
    const { navidrome, navidromeStatusKnown } = await import('../stores/navidrome.svelte');
    navidrome.state = 'off';
    navidromeStatusKnown.loaded = false;
    const view = await artistView([artist, ndArtist]);
    view.heart.click();
    flushSync();
    expect(setCalls()).toEqual([
      { kind: 'artist', id: 'Lumen Field', on: true },
      { kind: 'artist', id: 'navidrome:ar1', on: true },
    ]);
    unmount(view.app);
  });

  it('plays only the albums of the shown favorite artists, by name and source', async () => {
    const localAlbum = { ...album, id: 'a1', source: 'local' as const };
    const ndAlbum = { ...album, id: 'navidrome:al1', source: 'navidrome' as const };
    // Only the local "Lumen Field" is a favorite; the name also exists in Navidrome.
    const app = await favoritesView([artist]);
    const base = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (cmd, args) =>
      cmd === 'library_artist_albums' ? [localAlbum, ndAlbum] : base(cmd, args),
    );
    const play = [...target.querySelectorAll<HTMLButtonElement>('.btn.primary')][0];
    play.click();
    await settle();
    const albumCalls = invoke.mock.calls.filter(([cmd]) => cmd === 'library_album_tracks').map(([, a]) => a);
    expect(albumCalls).toEqual([{ albumId: 'a1' }]);
    unmount(app);
  });

  it('shows the not-connected text, never the raw error, when a heart is refused', async () => {
    const { setFavorite } = await import('../stores/favorites.svelte');
    const { notice } = await import('../stores/notice.svelte');
    // Tauri rejects a command with the error string itself.
    const base = invoke.getMockImplementation()!;
    restore = () => invoke.mockImplementation(base);
    invoke.mockImplementation(async (cmd, args) => {
      if (cmd === 'favorites_set') throw 'Navidrome is not connected';
      return base(cmd, args);
    });
    await setFavorite('track', 'navidrome:s1', true);
    expect(notice.text).toBe('Navidrome is not connected. Sign in again in the settings.');
  });

  it('offline_error_shows_one_notice', async () => {
    const { listen } = await import('@tauri-apps/api/event');
    const { followPlayerErrors } = await import('../stores/player.svelte');
    const { notice } = await import('../stores/notice.svelte');
    let emit: ((event: { payload: unknown }) => void) | undefined;
    vi.mocked(listen).mockImplementationOnce(async (name, cb) => {
      expect(name).toBe('player://error');
      emit = cb as typeof emit;
      return () => {};
    });
    vi.useFakeTimers();
    const stop = await followPlayerErrors();
    const offline = { trackId: 'navidrome:s1', message: 'Navidrome is not reachable', kind: 'offline' };

    emit!({ payload: offline });
    expect(notice.text).toBe('Navidrome is not reachable. Press Play once it is back.');
    // The same notice is not stacked or restarted by a second report while it shows.
    emit!({ payload: offline });
    expect(notice.text).toBe('Navidrome is not reachable. Press Play once it is back.');
    vi.advanceTimersByTime(5000);
    expect(notice.text).toBeNull();

    // A track error shows nothing, as in M2: the player moves on by itself.
    emit!({ payload: { trackId: 'local:t1', message: 'decode failed', kind: 'track' } });
    expect(notice.text).toBeNull();

    // A later drop is a new event with its own notice, in the UI language.
    setLang('de');
    emit!({ payload: offline });
    expect(notice.text).toBe('Navidrome ist nicht erreichbar. Später mit Abspielen fortsetzen.');
    stop();
  });

  it('labels the streamed quality in the transport strip and Now Playing', async () => {
    const { default: TransportStrip } = await import('./TransportStrip.svelte');
    const { default: NowPlaying } = await import('./NowPlaying.svelte');
    const { player } = await import('../stores/player.svelte');
    const { settings } = await import('../stores/settings.svelte');
    Object.assign(player, { status: 'playing', track: ndTrack() });
    const strip = mount(TransportStrip, { target });
    const np = mount(NowPlaying, { target });
    flushSync();
    const stripLabel = () => target.querySelector('.strip .quality') as HTMLElement;
    const npLabel = () => target.querySelector('.source .quality') as HTMLElement;
    expect(stripLabel().textContent?.trim()).toBe('FLAC 24/96');
    expect(npLabel().textContent?.trim()).toBe('Navidrome · FLAC 24/96');

    settings.navidromeQuality = '192';
    flushSync();
    expect(stripLabel().textContent?.trim()).toBe('MP3 192');
    expect(stripLabel().title).toBe('Streamed as MP3 192 from FLAC 24/96');
    expect(npLabel().textContent?.trim()).toBe('Navidrome · MP3 192 from FLAC 24/96');

    // A local track ignores the streaming quality.
    player.track = track(1);
    flushSync();
    expect(stripLabel().textContent?.trim()).toBe('FLAC 16/44.1');
    expect(stripLabel().title).toBe('');
    unmount(np);
    unmount(strip);
  });
});

// Sources: the sidebar entries, the source views, the source badges and the offline state.
describe('Sources', () => {
  const ndAlbum: Album = {
    ...album,
    id: 'navidrome:al1',
    source: 'navidrome',
    title: 'Night Signals',
    coverId: 'nd-al1',
  };
  const ndArtist: Artist = { ...artist, id: 'navidrome:ar1', source: 'navidrome', albumCount: 2, trackCount: 20 };
  const ndTrack = (n: number): Track => ({ ...track(n), id: `navidrome:s${n}`, source: 'navidrome', path: null });

  const status = (over: Partial<NavidromeStatus> = {}): NavidromeStatus => ({
    configured: true,
    url: null,
    user: 'demo',
    state: 'online',
    lastSync: 1,
    trackCount: 3,
    albumCount: 1,
    progress: null,
    error: null,
    ...over,
  });

  /** What the fake backend serves in this block. */
  let served = { status: status(), local: 3, navidrome: 3 };
  let impl: NonNullable<ReturnType<typeof invoke.getMockImplementation>>;

  beforeEach(async () => {
    served = { status: status(), local: 3, navidrome: 3 };
    impl = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (cmd, args) => {
      const source = (args as { source?: string } | undefined)?.source;
      switch (cmd) {
        case 'library_albums':
          return [album, ndAlbum].filter((a) => (a.source === 'local' ? served.local : served.navidrome) > 0);
        case 'library_artists':
          return [artist, ndArtist].filter((a) => (a.source === 'local' ? served.local : served.navidrome) > 0);
        case 'library_track_count':
          return source === 'navidrome' ? served.navidrome : source === 'local' ? served.local : served.local + served.navidrome;
        case 'library_tracks_range': {
          const { offset, limit } = args as { offset: number; limit: number };
          // Every source: local and Navidrome rows take turns.
          const pick = (n: number) =>
            source === 'navidrome' || (!source && n % 2 === 1) ? ndTrack(n) : track(n);
          return Array.from({ length: limit }, (_, i) => pick(offset + i));
        }
        case 'navidrome_status':
          return served.status;
        case 'library_search':
          return { albums: [ndAlbum], artists: [artist, ndArtist], tracks: [ndTrack(2)] };
      }
      return impl(cmd, args);
    });
    const { navidrome } = await import('../stores/navidrome.svelte');
    Object.assign(navidrome, served.status);
    const { go } = await import('../stores/nav.svelte');
    go('home');
  });

  afterEach(() => invoke.mockImplementation(impl));

  async function app(configure?: () => void) {
    configure?.();
    const { navidrome } = await import('../stores/navidrome.svelte');
    Object.assign(navidrome, served.status);
    const { default: App } = await import('../../App.svelte');
    const mounted = mount(App, { target });
    await vi.waitFor(async () => {
      await settle();
      expect(library.trackCounts?.all).toBe(served.local + served.navidrome);
    });
    await settle();
    return mounted;
  }

  const entry = (label: string) =>
    [...target.querySelectorAll<HTMLElement>('.sidebar .item')].find((b) => b.querySelector('.label')?.textContent === label);
  const cardTitles = () => [...target.querySelectorAll('.card .title .text')].map((el) => el.textContent);

  it('navidrome_entry_is_enabled_and_opens_its_albums', async () => {
    const mounted = await app();
    const item = entry('Navidrome')!;
    expect(item.getAttribute('aria-disabled')).toBeNull();
    expect(item.textContent).not.toContain('Coming soon');
    item.click();
    await settle();
    expect(nav.view).toBe('albums');
    expect(nav.source).toBe('navidrome');
    expect(target.querySelector('.content h1')?.textContent).toBe('Navidrome');
    expect(entry('Navidrome')!.getAttribute('aria-current')).toBe('page');
    // The Library entries are lit only for the views of every source.
    expect(entry('Albums')!.getAttribute('aria-current')).toBeNull();
    // A switch between the source's albums, artists and songs.
    const radios = [...target.querySelectorAll<HTMLElement>('.content [role="radio"]')];
    expect(radios.map((r) => r.textContent?.trim())).toEqual(['Albums', 'Artists', 'Songs']);
    radios[2].click();
    await settle();
    expect(nav.view).toBe('songs');
    expect(nav.source).toBe('navidrome');
    unmount(mounted);
  });

  it('has no YouTube placeholder in the sidebar', async () => {
    const mounted = await app();
    expect(entry('YouTube Music')).toBeUndefined();
    unmount(mounted);
  });

  it('local_entry_passes_source_local', async () => {
    const mounted = await app();
    entry('Local files')!.click();
    await settle();
    expect(nav.view).toBe('albums');
    expect(nav.source).toBe('local');
    expect(cardTitles()).toEqual(['Glass Tides']);
    expect(entry('Local files')!.getAttribute('aria-current')).toBe('page');
    entry('Albums')!.click();
    await settle();
    expect(nav.source).toBeUndefined();
    expect(entry('Albums')!.getAttribute('aria-current')).toBe('page');
    expect(entry('Local files')!.getAttribute('aria-current')).toBeNull();
    unmount(mounted);
  });

  it('navidrome_view_lists_only_navidrome_albums', async () => {
    const mounted = await app();
    entry('Albums')!.click();
    await settle();
    expect(cardTitles()).toEqual(['Glass Tides', 'Night Signals']);
    entry('Navidrome')!.click();
    await settle();
    expect(cardTitles()).toEqual(['Night Signals']);
    expect(target.querySelector('.content .viewhead .meta')?.textContent).toBe('1 album');
    unmount(mounted);
  });

  it('songs_view_requests_its_source', async () => {
    library.trackCounts = { all: 330, local: 30, navidrome: 300 };
    library.version++;
    const mounted = mount(SongList, { target, props: { source: 'navidrome' } });
    await frame();
    await settle();
    const ranges = invoke.mock.calls.filter(([cmd]) => cmd === 'library_tracks_range');
    expect(ranges).toEqual([['library_tracks_range', { offset: 0, limit: 200, source: 'navidrome' }]]);
    expect((target.querySelector('[role="listbox"]') as HTMLElement).style.height).toBe(`${300 * 44}px`);
    // A double-click queues the rows of the same source.
    target.querySelectorAll('[role="option"]')[2].dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    await settle();
    expect(invoke).toHaveBeenCalledWith('library_tracks_range', { offset: 0, limit: 1000, source: 'navidrome' });
    const play = invoke.mock.calls.find(([cmd]) => cmd === 'player_play');
    expect((play?.[1] as { trackIds: string[] }).trackIds[2]).toBe('navidrome:s2');
    unmount(mounted);
  });

  it('badges_in_all_views_not_in_source_views', async () => {
    const badges = () => [...target.querySelectorAll<HTMLElement>('.source-badge')];
    const mounted = await app();
    entry('Albums')!.click();
    await settle();
    // Only streamed items carry a dot; local files stay unmarked.
    expect(badges().map((b) => b.getAttribute('aria-label'))).toEqual(['Navidrome']);
    expect(badges()[0].getAttribute('title')).toBe('Navidrome');
    expect(badges()[0].dataset.source).toBe('navidrome');
    entry('Navidrome')!.click();
    await settle();
    expect(badges()).toHaveLength(0);

    // Songs: on in the list of every source, off in a source's own list.
    const { go } = await import('../stores/nav.svelte');
    go('songs');
    await frame();
    await settle();
    const rows = [...target.querySelectorAll('[role="option"]')];
    expect(rows[0].querySelector('.source-badge')).toBeNull();
    expect(rows[1].querySelector('.source-badge')?.getAttribute('aria-label')).toBe('Navidrome');
    go('songs', undefined, 'navidrome');
    await frame();
    await settle();
    expect(target.querySelector('[role="option"]')).not.toBeNull();
    expect(badges()).toHaveLength(0);
    unmount(mounted);

    // The search palette marks every result with its source.
    vi.useFakeTimers();
    const { app: open, input } = palette();
    await search(input, 'night');
    expect(badges().map((b) => b.getAttribute('aria-label'))).toEqual(['Navidrome', 'Navidrome', 'Navidrome']);
    unmount(open);
  });

  it('not_configured_navidrome_offers_settings', async () => {
    const mounted = await app(() => {
      served.status = status({ configured: false, state: 'off', user: null, trackCount: 0, albumCount: 0, lastSync: null });
      served.navidrome = 0;
    });
    entry('Navidrome')!.click();
    await settle();
    expect(target.querySelector('.content h1')?.textContent).toBe('Connect your Navidrome server');
    const button = [...target.querySelectorAll<HTMLButtonElement>('.content button')].find((b) =>
      b.textContent?.includes('Open settings'),
    )!;
    button.click();
    await settle();
    expect(nav.view).toBe('settings');
    unmount(mounted);
  });

  it('shows the first sync with its progress', async () => {
    const mounted = await app(() => {
      served.status = status({ state: 'syncing', lastSync: null, trackCount: 0, progress: { done: 12_000, total: 31_204 } });
      served.navidrome = 0;
      served.local = 0;
    });
    entry('Navidrome')!.click();
    await settle();
    expect(target.querySelector('.content h1')?.textContent).toBe('Syncing your Navidrome library');
    expect(target.querySelector('.content .progress .label')?.textContent).toContain('12,000 of 31,204 songs');
    // The live region carries the state, not the moving count.
    const live = target.querySelector('.content [role="status"]')!;
    expect(live.textContent).toBe('Syncing your Navidrome library');
    expect(target.querySelector('.content .progress [aria-live]')).toBeNull();
    expect(target.querySelector('.content button.local')).toBeNull();
    // With no local files yet, the views of every source wait for the sync too, and still
    // lead to local music.
    entry('Home')!.click();
    await settle();
    expect(target.querySelector('.content h1')?.textContent).toBe('Syncing your Navidrome library');
    target.querySelector<HTMLButtonElement>('.content button.local')!.click();
    await settle();
    expect(nav.view).toBe('albums');
    expect(nav.source).toBe('local');
    expect(target.querySelector('.content h1')?.textContent).toBe('Your library is empty');
    unmount(mounted);
  });

  it('says calmly that an empty index waits for the server while offline', async () => {
    const mounted = await app(() => {
      served.status = status({ state: 'offline', trackCount: 0 });
      served.navidrome = 0;
    });
    entry('Navidrome')!.click();
    await settle();
    expect(target.querySelector('.content h1')?.textContent).toBe('Navidrome is not reachable');
    unmount(mounted);
  });

  it('offers a sync on an empty server', async () => {
    const mounted = await app(() => {
      served.status = status({ trackCount: 0 });
      served.navidrome = 0;
    });
    entry('Navidrome')!.click();
    await settle();
    expect(target.querySelector('.content h1')?.textContent).toBe('No music on this server yet');
    [...target.querySelectorAll<HTMLButtonElement>('.content button')].find((b) => b.textContent?.includes('Sync now'))!.click();
    await settle();
    expect(invoke).toHaveBeenCalledWith('navidrome_sync', undefined);
    unmount(mounted);
  });

  it('keeps the index browsable while offline, with a hint', async () => {
    const mounted = await app(() => {
      served.status = status({ state: 'offline' });
    });
    entry('Navidrome')!.click();
    await settle();
    expect(cardTitles()).toEqual(['Night Signals']);
    // A short visible label; the full sentence is the tooltip and the accessible name.
    const hint = target.querySelector<HTMLElement>('.content .source-hint')!;
    expect(hint.textContent?.trim()).toBe('Offline');
    expect(hint.getAttribute('title')).toBe('Offline, showing the saved library');
    expect(hint.getAttribute('aria-label')).toBe('Offline, showing the saved library');
    expect(hint.getAttribute('aria-live')).toBeNull();
    unmount(mounted);
  });

  it('offline_dot_has_the_offline_label', async () => {
    const { navidrome } = await import('../stores/navidrome.svelte');
    const mounted = await app(() => {
      served.status = status({ state: 'offline' });
    });
    const item = entry('Navidrome')!;
    expect(item.getAttribute('aria-label')).toBe('Navidrome (offline)');
    expect(item.querySelector('.dot')!.classList.contains('hollow')).toBe(true);
    navidrome.state = 'error';
    navidrome.error = { code: 'wrongCredentials', detail: null };
    flushSync();
    expect(item.getAttribute('aria-label')).toBe('Navidrome (sync failed)');
    expect(item.querySelector('.dot')!.classList.contains('hollow')).toBe(true);
    // State changes are announced once, in one live region outside the view.
    const live = target.querySelector('.main > [role="status"]')!;
    expect(live.textContent).toBe('Sync failed, showing the saved library');
    navidrome.state = 'syncing';
    flushSync();
    expect(live.textContent).toBe('Navidrome sync started');
    navidrome.progress = { done: 10, total: 600 };
    flushSync();
    expect(live.textContent).toBe('Navidrome sync started');
    navidrome.state = 'online';
    navidrome.progress = null;
    navidrome.error = null;
    flushSync();
    expect(live.textContent).toBe('Navidrome sync finished');
    expect(item.getAttribute('aria-label')).toBeNull();
    expect(item.querySelector('.dot')!.classList.contains('hollow')).toBe(false);
    unmount(mounted);
  });

  it('artist_rows_keyed_by_id', async () => {
    const { default: ArtistList } = await import('./ArtistList.svelte');
    const mounted = mount(ArtistList, { target, props: { artists: [artist, ndArtist], showSource: true } });
    flushSync();
    const rows = [...target.querySelectorAll('.list li')];
    expect(rows).toHaveLength(2);
    expect(rows.map((r) => r.querySelector('.name')?.textContent)).toEqual(['Lumen Field', 'Lumen Field']);
    // Two artists of one name read apart: the streamed one has a badge, both name their
    // source in the counts line.
    expect(rows.map((r) => r.querySelector('.source-badge')?.getAttribute('aria-label'))).toEqual([undefined, 'Navidrome']);
    expect(rows[0].querySelector('.counts')?.textContent).toContain('Local files');
    expect(rows[1].querySelector('.counts')?.textContent).toContain('Navidrome');
    // Both open the one artist page, a union over the sources.
    rows[1].querySelector<HTMLButtonElement>('button')!.click();
    expect(nav.view).toBe('artist');
    expect(nav.param).toBe('Lumen Field');
    unmount(mounted);
  });
});
