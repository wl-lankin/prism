import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { Album, Artist, SavedStations, Station, Track } from '../types';

const station = (n: number, name = `Test FM ${n}`): Station => ({
  id: `radio:00000000-0000-4000-8000-${String(n).padStart(12, '0')}`,
  name,
  url: `http://127.0.0.1:1/${n}`,
  urlResolved: null,
  homepage: null,
  favicon: null,
  coverId: null,
  countryCode: 'DE',
  region: 'Testland',
  language: null,
  tags: [],
  codec: 'MP3',
  bitrateKbps: 128,
  hls: false,
  lastCheckOk: true,
  distanceKm: null,
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
const artist: Artist = { id: 'Lumen Field', source: 'local', name: 'Lumen Field', albumCount: 1, trackCount: 3, coverId: null };
const song: Track = {
  id: 'local:t2',
  source: 'local',
  title: 'Song 2',
  artist: 'Lumen Field',
  albumArtist: 'Lumen Field',
  album: 'Glass Tides',
  albumId: 'a1',
  trackNo: 2,
  discNo: 1,
  durationMs: 180_000,
  year: 2021,
  genre: null,
  format: 'flac',
  sampleRate: 44_100,
  bitDepth: 16,
  bitrateKbps: null,
  coverId: null,
  path: 'C:/demo/2.flac',
  replayGainTrackDb: null,
  replayGainAlbumDb: null,
  replayGainTrackPeak: null,
  replayGainAlbumPeak: null,
};

let saved: SavedStations = { favorites: [], recent: [] };
let directory: Station[] | 'down' = [];
let playResult: 'ok' | string = 'ok';

const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
  switch (cmd) {
    case 'library_search':
      if ((args as { q: string }).q === 'zz') return { albums: [], artists: [], tracks: [] };
      return { albums: [album], artists: [artist], tracks: [song] };
    case 'favorites_tracks':
    case 'favorites_albums':
    case 'favorites_artists':
      return [];
    case 'radio_saved':
      return saved;
    case 'radio_search':
      if (directory === 'down') throw { code: 'unreachable' };
      return directory;
    case 'radio_play':
      if (playResult !== 'ok') throw playResult;
      return undefined;
    default:
      return undefined;
  }
});
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= function () {};
Element.prototype.getAnimations ??= () => [];

const { default: SearchPalette } = await import('./SearchPalette.svelte');
const { default: FavoritesView } = await import('./FavoritesView.svelte');
const { default: HomeStations } = await import('./HomeStations.svelte');
const { radio, loadSaved } = await import('../stores/radio.svelte');
const { favorites } = await import('../stores/favorites.svelte');
const { notice } = await import('../stores/notice.svelte');
const { setLang } = await import('../i18n');

let target: HTMLElement;
beforeEach(() => {
  setLang('en');
  invoke.mockClear();
  saved = { favorites: [], recent: [] };
  directory = [];
  playResult = 'ok';
  radio.saved = { favorites: [], recent: [] };
  favorites.stations.clear();
  notice.text = null;
  target = document.createElement('div');
  document.body.append(target);
});
afterEach(() => {
  target.remove();
  vi.useRealTimers();
});

async function settle() {
  for (let i = 0; i < 6; i++) {
    await tick();
    await Promise.resolve();
  }
  flushSync();
}

function type(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
}

function key(el: Element, k: string) {
  el.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true }));
  flushSync();
}

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

/** Saves these as the favorites, the way the backend would report them. */
async function favoriteStations(...list: Station[]) {
  saved = { favorites: list, recent: [] };
  for (const item of list) favorites.stations.add(item.id);
  await loadSaved();
}

const headings = () => [...target.querySelectorAll('h2')].map((h) => h.textContent);
const group = (name: string) =>
  [...target.querySelectorAll('[role="group"]')].find((g) => g.querySelector('h2')?.textContent === name);
const names = (g: Element | undefined) => [...(g?.querySelectorAll('.main') ?? [])].map((x) => x.textContent);
const chip = (name: string) =>
  [...target.querySelectorAll<HTMLButtonElement>('.chip')].find((x) => x.textContent?.trim() === name);

it('favorites_lists_stations_and_radio_chip', async () => {
  const app = mount(FavoritesView, { target });
  await settle();
  expect(chip('Radio')).toBeUndefined();
  expect(target.textContent).not.toContain('Test FM 1');
  unmount(app);

  await favoriteStations(station(1), station(2));
  const view = mount(FavoritesView, { target });
  await settle();
  expect(chip('Radio')).toBeTruthy();
  expect(target.querySelector('#fav-stations h2')?.textContent).toContain('Stations');
  expect([...target.querySelectorAll('#fav-stations .station .name')].map((x) => x.textContent)).toEqual([
    'Test FM 1',
    'Test FM 2',
  ]);

  // The Radio chip leaves only the stations; another source hides them.
  chip('Radio')!.click();
  await settle();
  expect(target.querySelectorAll('#fav-stations .station')).toHaveLength(2);
  chip('Local files')!.click();
  await settle();
  expect(target.querySelector('#fav-stations')).toBeNull();

  // A station un-hearted here leaves the list at once.
  chip('All')!.click();
  await settle();
  target.querySelector<HTMLButtonElement>('#fav-stations .station .heart')!.click();
  await settle();
  expect(invoke).toHaveBeenCalledWith('radio_favorite', { station: station(1), on: false });
  expect(target.querySelectorAll('#fav-stations .station')).toHaveLength(1);

  // A row plays its station and a failure names it.
  playResult = 'The station is not reachable';
  target.querySelector<HTMLButtonElement>('#fav-stations .station .open')!.click();
  await settle();
  expect(invoke).toHaveBeenCalledWith('radio_play', { station: station(2) });
  expect(notice.text).toBe('Could not play Test FM 2: The station is not reachable');
  unmount(view);
});

it('home_row_only_with_station_favorites', async () => {
  const app = mount(HomeStations, { target });
  await settle();
  expect(target.querySelector('.stations')).toBeNull();
  unmount(app);

  await favoriteStations(...Array.from({ length: 14 }, (_, i) => station(i + 1)));
  const row = mount(HomeStations, { target });
  await settle();
  expect(target.querySelector('h2')?.textContent).toBe('Your stations');
  const tiles = [...target.querySelectorAll<HTMLButtonElement>('.tile')];
  expect(tiles).toHaveLength(12);
  expect(tiles[0].textContent).toContain('Test FM 1');

  tiles[2].click();
  await settle();
  expect(invoke).toHaveBeenCalledWith('radio_play', { station: station(3) });

  // Taking every heart off removes the row.
  favorites.stations.clear();
  await settle();
  expect(target.querySelector('.stations')).toBeNull();
  unmount(row);
});

it('palette_shows_saved_stations_first', async () => {
  vi.useFakeTimers();
  saved = { favorites: [station(1, 'Alpha FM')], recent: [station(2, 'Beta FM')] };
  directory = [station(1, 'Alpha FM'), station(3, 'Gamma FM'), station(4, 'Delta FM')];
  await loadSaved();
  const { app, input } = palette();
  type(input, 'fm');
  // Saved stations match before any request is made.
  expect(names(group('Stations'))).toEqual(['Alpha FM', 'Beta FM']);
  expect(invoke.mock.calls.some(([cmd]) => cmd === 'radio_search')).toBe(false);

  vi.advanceTimersByTime(120);
  await settle();
  expect(headings()).toEqual(['Albums', 'Artists', 'Songs', 'Stations']);
  expect(invoke.mock.calls.some(([cmd]) => cmd === 'radio_search')).toBe(false);

  vi.advanceTimersByTime(180);
  await settle();
  expect(invoke).toHaveBeenCalledWith('radio_search', { query: { name: 'fm', limit: 8 } });
  // Saved first, then the directory's, nothing twice.
  expect(names(group('Stations'))).toEqual(['Alpha FM', 'Beta FM', 'Gamma FM', 'Delta FM']);
  unmount(app);
});

it('palette_hides_stations_on_directory_error', async () => {
  vi.useFakeTimers();
  directory = 'down';
  const { app, input } = palette();
  type(input, 'fm');
  vi.advanceTimersByTime(300);
  await settle();
  expect(headings()).toEqual(['Albums', 'Artists', 'Songs']);
  expect(target.querySelector('.note')).toBeNull();

  // Saved stations still answer while the directory is down.
  saved = { favorites: [station(1, 'Alpha FM')], recent: [] };
  await loadSaved();
  await settle();
  expect(names(group('Stations'))).toEqual(['Alpha FM']);
  unmount(app);
});

it('palette_enter_plays_a_station', async () => {
  vi.useFakeTimers();
  saved = { favorites: [station(1, 'zz Radio')], recent: [] };
  await loadSaved();
  const { app, state, input } = palette();
  type(input, 'zz');
  vi.advanceTimersByTime(120);
  await settle();
  expect(target.querySelector('[aria-selected="true"]')?.textContent).toContain('zz Radio');
  key(input, 'Enter');
  await settle();
  expect(invoke).toHaveBeenCalledWith('radio_play', { station: saved.favorites[0] });
  expect(state.open).toBe(false);
  unmount(app);
});

it('home_row_shows_on_an_empty_library', async () => {
  await favoriteStations(station(1));
  const { default: App } = await import('../../App.svelte');
  const app = mount(App, { target });
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelector('#home-stations')?.textContent).toBe('Your stations');
  });
  unmount(app);
});
