import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
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

describe('Navidrome playlists', () => {
  const ndTrack = (n: number): Track => ({ ...track(n), id: `navidrome:s${n}`, source: 'navidrome', path: null });
  const remote = {
    id: 'navidrome:p1',
    name: 'Night drive',
    owner: 'demo',
    trackCount: 3,
    durationMs: 360_000,
    coverIds: ['nd-c1', 'nd-c2'],
    updatedAt: 5,
  };
  /** What `navidrome_playlists` serves; empty means no server playlists. */
  let remotes: (typeof remote)[] = [];
  /** What `playlist_entries` serves instead of the default, when set. */
  let localEntries: unknown[] | null = null;
  /** What `playlists_list` serves instead of the default, when set. */
  let localList: unknown[] | null = null;
  /** What `navidrome_playlist_entries` serves instead of the default, when set. */
  let remoteEntries: unknown[] | null = null;

  let impl: NonNullable<ReturnType<typeof invoke.getMockImplementation>>;
  beforeEach(() => {
    remotes = [remote];
    localEntries = null;
    localList = null;
    remoteEntries = null;
    impl = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (cmd, args) => {
      if (cmd === 'navidrome_playlists') return remotes;
      if (cmd === 'navidrome_playlist_entries')
        return (
          remoteEntries ?? [
            { position: 0, trackId: 'navidrome:s1', track: ndTrack(1) },
            { position: 1, trackId: 'navidrome:s2', track: null },
            { position: 2, trackId: 'navidrome:s3', track: ndTrack(3) },
          ]
        );
      if (cmd === 'playlists_list' && localList) return localList;
      if (cmd === 'playlist_entries' && localEntries) return localEntries;
      return impl(cmd, args);
    });
  });
  afterEach(async () => {
    invoke.mockImplementation(impl);
    const { reloadPlaylists } = await import('../stores/playlists.svelte');
    await reloadPlaylists();
  });

  async function playlistsView() {
    const { default: PlaylistsView } = await import('./PlaylistsView.svelte');
    const { reloadPlaylists } = await import('../stores/playlists.svelte');
    await reloadPlaylists();
    const app = mount(PlaylistsView, { target });
    await settle();
    return app;
  }

  async function remoteView() {
    const { default: PlaylistView } = await import('./PlaylistView.svelte');
    const { reloadPlaylists } = await import('../stores/playlists.svelte');
    await reloadPlaylists();
    const app = mount(PlaylistView, { target, props: { playlistId: 'navidrome:p1' } });
    await settle();
    return { app, list: target.querySelector<HTMLElement>('[role="listbox"]')! };
  }

  const menuItem = (label: string) =>
    [...target.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
      (el) => el.querySelector('.label')?.textContent?.trim() === label,
    );

  it('navidrome_section_lists_remote_playlists', async () => {
    const app = await playlistsView();
    const section = target.querySelector<HTMLElement>('section.remote')!;
    expect(section).not.toBeNull();
    expect(section.querySelector('h2')?.textContent).toContain('Navidrome');
    expect(section.querySelector('[data-source="navidrome"]')).not.toBeNull();
    const card = section.querySelector<HTMLElement>('.card')!;
    expect(card.textContent).toContain('Night drive');
    expect(card.querySelector('.readonly')?.textContent).toContain('Read-only');
    // The Prism playlists stay above, editable as before.
    expect(target.querySelector('.grid:not(.remote .grid)')?.textContent).toContain('Road trip');
    card.querySelector<HTMLElement>('.open')!.click();
    flushSync();
    expect(nav.view).toBe('playlist');
    expect(nav.param).toBe('navidrome:p1');
    unmount(app);

    remotes = [];
    const none = await playlistsView();
    expect(target.querySelector('section.remote')).toBeNull();
    unmount(none);
  });

  it('plays a remote playlist from its card, skipping unavailable entries', async () => {
    const app = await playlistsView();
    invoke.mockClear();
    target.querySelector<HTMLElement>('section.remote .play')!.click();
    await settle();
    expect(invoke).toHaveBeenCalledWith('navidrome_playlist_entries', { id: 'navidrome:p1' });
    expect(invoke).toHaveBeenCalledWith('player_play', { trackIds: ['navidrome:s1', 'navidrome:s3'], startIndex: 0 });
    unmount(app);
  });

  it('remote_playlist_is_read_only', async () => {
    const { app, list } = await remoteView();
    expect(invoke).toHaveBeenCalledWith('navidrome_playlist_entries', { id: 'navidrome:p1' });
    expect(invoke).not.toHaveBeenCalledWith('playlist_entries', expect.anything());
    expect(target.querySelector('h1')?.textContent).toContain('Night drive');
    expect(target.querySelector('.rename')).toBeNull();
    expect(target.querySelector('[aria-label="Delete playlist"]')).toBeNull();
    expect(target.querySelector('.grip')).toBeNull();
    expect(target.querySelector('.readonly')?.textContent).toContain('Read-only');
    expect(target.querySelector('.meta')?.textContent).toContain('demo');
    expect(list.getAttribute('aria-keyshortcuts')).not.toContain('Delete');
    expect(list.getAttribute('aria-readonly')).toBe('true');
    // The read-only hint reaches screen readers, not only as a tooltip.
    const hint = document.getElementById(list.getAttribute('aria-describedby')!);
    expect(hint?.textContent).toContain('belongs to your Navidrome server');
    expect(target.querySelector('.readonly')?.getAttribute('aria-describedby')).toBe(hint?.id);

    // The unavailable entry is dimmed and labelled, not an error.
    const rows = target.querySelectorAll('[role="option"]');
    expect(rows).toHaveLength(3);
    expect(rows[1].classList.contains('missing')).toBe(true);
    expect(rows[1].textContent).toContain('Not available from Navidrome');
    expect(target.textContent).toContain('1 unavailable');

    invoke.mockClear();
    key(list, 'ArrowDown');
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true }));
    key(list, 'Delete');
    // Dragging does nothing either.
    const at = { bubbles: true, button: 0, pointerId: 1, clientY: 100 };
    rows[0].querySelector('.name')!.dispatchEvent(new PointerEvent('pointerdown', at));
    list.dispatchEvent(new PointerEvent('pointermove', { ...at, clientY: 160 }));
    flushSync();
    expect(list.classList.contains('dragging')).toBe(false);
    list.dispatchEvent(new PointerEvent('pointerup', at));
    await settle();
    expect(invoke).not.toHaveBeenCalledWith('playlist_move', expect.anything());
    expect(invoke).not.toHaveBeenCalledWith('playlist_remove', expect.anything());
    expect(target.querySelectorAll('[role="option"]')).toHaveLength(3);

    // The menu offers no removal.
    rows[0].dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 50, clientY: 80 }));
    await settle();
    expect(target.querySelector('[role="menu"]')).not.toBeNull();
    expect(menuItem('Remove from playlist')).toBeUndefined();
    unmount(app);
  });

  it('remote_playlist_plays_its_entries', async () => {
    const { app, list } = await remoteView();
    invoke.mockClear();
    [...target.querySelectorAll<HTMLButtonElement>('.actions .btn')].find((b) => b.textContent?.includes('Play'))!.click();
    await settle();
    expect(invoke).toHaveBeenCalledWith('player_play', { trackIds: ['navidrome:s1', 'navidrome:s3'], startIndex: 0 });

    invoke.mockClear();
    target.querySelectorAll('[role="option"]')[2].dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    await settle();
    expect(invoke).toHaveBeenCalledWith('player_play', { trackIds: ['navidrome:s1', 'navidrome:s3'], startIndex: 1 });

    // An unavailable row plays nothing.
    invoke.mockClear();
    const missing = target.querySelectorAll('[role="option"]')[1];
    missing.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    missing.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    flushSync();
    key(list, 'Enter');
    await settle();
    expect(invoke).not.toHaveBeenCalledWith('player_play', expect.anything());

    // Shuffle turns the shuffle mode on and plays only the available entries.
    invoke.mockClear();
    [...target.querySelectorAll<HTMLButtonElement>('.actions .btn')].find((b) => b.textContent?.includes('Shuffle'))!.click();
    await settle();
    const play = invoke.mock.calls.find(([cmd]) => cmd === 'player_play');
    expect((play?.[1] as { trackIds: string[] }).trackIds).toEqual(['navidrome:s1', 'navidrome:s3']);
    unmount(app);
  });

  it('remote_tracks_can_go_into_a_prism_playlist', async () => {
    const { app } = await remoteView();
    const row = target.querySelectorAll('[role="option"]')[0];
    row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 50, clientY: 80 }));
    await settle();
    menuItem('Add to playlist')!.click();
    flushSync();
    // Only Prism playlists are offered: a server playlist takes nothing.
    expect(menuItem('Night drive')).toBeUndefined();
    menuItem('Road trip')!.click();
    await settle();
    expect(invoke).toHaveBeenCalledWith('playlist_add', { id: 1, trackIds: ['navidrome:s1'] });
    unmount(app);
  });

  it('shows Navidrome entries of a Prism playlist as unavailable while they are not in the index', async () => {
    localEntries = [
      { position: 0, trackId: 'local:t1', track: track(1) },
      { position: 1, trackId: 'navidrome:s9', track: null },
      { position: 2, trackId: 'local:t3', track: track(3) },
    ];
    const { default: PlaylistView } = await import('./PlaylistView.svelte');
    const { reloadPlaylists } = await import('../stores/playlists.svelte');
    await reloadPlaylists();
    const app = mount(PlaylistView, { target, props: { playlistId: 1 } });
    await settle();
    const rows = target.querySelectorAll('[role="option"]');
    expect(rows[1].classList.contains('missing')).toBe(true);
    expect(rows[1].textContent).toContain('Not available from Navidrome');
    // Still a Prism playlist: renaming and removing stay.
    expect(target.querySelector('.rename')).not.toBeNull();
    invoke.mockClear();
    rows[2].dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    await settle();
    expect(invoke).toHaveBeenCalledWith('player_play', { trackIds: ['local:t1', 'local:t3'], startIndex: 1 });
    unmount(app);
  });

  it('counts every playlist in the header and words the empty state for Prism playlists', async () => {
    const app = await playlistsView();
    expect(target.textContent).toContain('3 playlists');
    // The card's hint is tied to it for screen readers.
    const open = target.querySelector<HTMLElement>('section.remote .open')!;
    expect(document.getElementById(open.getAttribute('aria-describedby')!)?.textContent).toContain(
      'belongs to your Navidrome server',
    );
    unmount(app);

    localList = [];
    const empty = await playlistsView();
    expect(target.querySelector('.empty h2')?.textContent).toBe('No Prism playlists yet');
    expect(target.textContent).toContain('1 playlist');
    expect(target.querySelector('section.remote')).not.toBeNull();
    unmount(empty);

    remotes = [];
    const none = await playlistsView();
    expect(target.querySelector('.empty h2')?.textContent).toBe('No playlists yet');
    unmount(none);
  });

  it('says when nothing in a remote playlist is available yet', async () => {
    const { notice } = await import('../stores/notice.svelte');
    remoteEntries = [
      { position: 0, trackId: 'navidrome:s1', track: null },
      { position: 1, trackId: 'navidrome:s2', track: null },
    ];
    const app = await playlistsView();
    invoke.mockClear();
    target.querySelector<HTMLElement>('section.remote .play')!.click();
    await settle();
    expect(invoke).not.toHaveBeenCalledWith('player_play', expect.anything());
    expect(notice.text).toBe('None of these songs are available from Navidrome yet.');
    notice.text = null;
    unmount(app);
  });

  it('still moves and removes an unavailable entry of a Prism playlist', async () => {
    const { default: PlaylistView } = await import('./PlaylistView.svelte');
    const { reloadPlaylists } = await import('../stores/playlists.svelte');
    await reloadPlaylists();
    const app = mount(PlaylistView, { target, props: { playlistId: 1 } });
    await settle();
    const list = target.querySelector<HTMLElement>('[role="listbox"]')!;
    expect(list.getAttribute('aria-readonly')).toBeNull();
    const titles = () => [...list.querySelectorAll('[role="option"] .title')].map((el) => el.textContent);
    // Row 1 is the entry that is not in the library.
    key(list, 'ArrowDown');
    key(list, 'ArrowDown');
    expect(list.getAttribute('aria-activedescendant')).toBe('pl-row-1');
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true }));
    flushSync();
    expect(titles()).toEqual(['Song 1', 'Song 3', 'Not in your library']);
    key(list, 'Delete');
    await settle();
    expect(invoke).toHaveBeenCalledWith('playlist_move', { id: 1, from: 1, to: 2 });
    expect(invoke).toHaveBeenCalledWith('playlist_remove', { id: 1, positions: [2] });
    unmount(app);
  });

  it('says so when the server playlist is gone', async () => {
    const { default: PlaylistView } = await import('./PlaylistView.svelte');
    const { reloadPlaylists } = await import('../stores/playlists.svelte');
    remotes = [];
    await reloadPlaylists();
    const app = mount(PlaylistView, { target, props: { playlistId: 'navidrome:p1' } });
    await settle();
    expect(target.textContent).toContain('This playlist no longer exists.');
    unmount(app);
  });
});
