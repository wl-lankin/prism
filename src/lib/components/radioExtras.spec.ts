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

const tabButton = (kind: string) => target.querySelector<HTMLButtonElement>(`#fav-tab-${kind}`)!;
async function openTab(kind: string) {
  tabButton(kind).click();
  await settle();
}
const chipNames = () => [...target.querySelectorAll('.chip')].map((x) => x.textContent?.trim());

it('favorites_stations_tab_lists_stations', async () => {
  await favoriteStations(station(1), station(2));
  const view = mount(FavoritesView, { target });
  await settle();
  await openTab('track');

  // The stations are a fourth tab with a count, not a section under the header.
  expect([...target.querySelectorAll('[role="tab"]')].map((x) => x.id)).toEqual([
    'fav-tab-track',
    'fav-tab-album',
    'fav-tab-artist',
    'fav-tab-station',
  ]);
  expect(tabButton('station').textContent).toContain('Stations');
  expect(tabButton('station').querySelector('.count')?.textContent).toBe('2');
  expect(target.querySelector('#fav-stations')).toBeNull();
  expect(target.querySelector('.stations')).toBeNull();

  await openTab('station');
  expect(tabButton('station').getAttribute('aria-selected')).toBe('true');
  expect(target.querySelector('#fav-panel .stations [role="listbox"]')?.getAttribute('style') ?? '').not.toContain('height');
  expect([...target.querySelectorAll('#fav-panel .stations .row .name')].map((x) => x.textContent)).toEqual([
    'Test FM 1',
    'Test FM 2',
  ]);
  expect(target.querySelector('.viewhead .meta')?.textContent).toBe('2 stations');
  // Stations cannot be queued: no Play or Shuffle.
  expect(target.querySelector('.viewhead .actions')).toBeNull();
  // The songs empty state stays away on this tab.
  expect(target.textContent).not.toContain('No favorite songs yet');

  // A station un-hearted here leaves the list at once, through the shared heart.
  const heart = target.querySelector<HTMLButtonElement>('.stations .row .heart')!;
  expect(heart.getAttribute('aria-pressed')).toBe('true');
  expect(heart.getAttribute('aria-label')).toBe('Favorite: Test FM 1');
  heart.click();
  await settle();
  expect(invoke).toHaveBeenCalledWith('radio_favorite', { station: station(1), on: false });
  expect(target.querySelectorAll('.stations .row')).toHaveLength(1);
  expect(target.querySelector('.viewhead .meta')?.textContent).toBe('1 station');

  // A row plays its station and a failure names it.
  playResult = 'The station is not reachable';
  target.querySelector<HTMLButtonElement>('.stations .row [aria-label="Play Test FM 2"]')!.click();
  await settle();
  expect(invoke).toHaveBeenCalledWith('radio_play', { station: station(2) });
  expect(notice.text).toBe('Test FM 2 could not be played: The station is not reachable');

  // Back on songs, the queue buttons return.
  await openTab('track');
  expect(target.querySelector('.viewhead .actions')).not.toBeNull();
  unmount(view);
});

it('favorites_stations_tab_empty_state_finds_stations', async () => {
  const view = mount(FavoritesView, { target });
  await settle();
  await openTab('station');
  expect(tabButton('station').querySelector('.count')?.textContent).toBe('0');
  expect(target.querySelector('.empty h2')?.textContent).toBe('No favorite stations yet');
  expect(target.querySelector('.empty .lead')?.textContent).toBe(
    'Tap the heart next to a station or in the player bar to keep it here.',
  );
  expect(target.textContent).not.toContain('No favorite songs yet');
  expect(target.querySelectorAll('.empty')).toHaveLength(1);
  expect(target.querySelector('.viewhead .actions')).toBeNull();

  const find = [...target.querySelectorAll<HTMLButtonElement>('.empty button')].find(
    (x) => x.textContent?.trim() === 'Find stations',
  )!;
  expect(find).toBeTruthy();
  find.click();
  await settle();
  const { nav, go } = await import('../stores/nav.svelte');
  expect(nav.view).toBe('radio');
  expect(nav.param).toBe('near');
  go('home');

  // The songs tab keeps its own invitation.
  await openTab('track');
  expect(target.querySelector('.empty h2')?.textContent).toBe('No favorite songs yet');
  unmount(view);
});

it('favorites_chips_follow_the_tab', async () => {
  await favoriteStations(station(1));
  const view = mount(FavoritesView, { target });
  await settle();
  await openTab('track');
  expect(chipNames()).toEqual(['All', 'Local files']);

  await openTab('station');
  expect(chipNames()).toEqual(['All', 'Radio']);
  chip('Radio')!.click();
  await settle();
  expect(chip('Radio')!.getAttribute('aria-checked')).toBe('true');
  expect(target.querySelectorAll('.stations .row')).toHaveLength(1);

  // Leaving the stations tab drops the Radio choice and the chip.
  await openTab('album');
  expect(chipNames()).toEqual(['All', 'Local files']);
  expect(chip('All')!.getAttribute('aria-checked')).toBe('true');

  // The keyboard model covers four tabs: End reaches stations, Home returns to songs.
  await openTab('track');
  key(target.querySelector('[role="tablist"]')!, 'End');
  await settle();
  expect(tabButton('station').getAttribute('aria-selected')).toBe('true');
  key(target.querySelector('[role="tablist"]')!, 'ArrowRight');
  await settle();
  expect(tabButton('track').getAttribute('aria-selected')).toBe('true');
  await openTab('track');
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
  const tiles = [...target.querySelectorAll<HTMLButtonElement>('button.station')];
  expect(tiles).toHaveLength(12);
  expect(tiles[0].textContent).toContain('Test FM 1');
  // The shared station tile: no logo, so the initials in radio blue.
  expect(tiles[0].querySelector('.tile .initials')?.textContent).toBe('TF');

  tiles[2].click();
  await settle();
  expect(invoke).toHaveBeenCalledWith('radio_play', { station: station(3) });

  // A failure names the station, with the radio view's notice.
  playResult = 'This station is not sending audio';
  tiles[3].click();
  await settle();
  expect(notice.text).toBe('Test FM 4 could not be played: This station is not sending audio');

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

it('palette_station_failure_names_the_station', async () => {
  vi.useFakeTimers();
  playResult = 'The station is not reachable';
  saved = { favorites: [station(1, 'zz Radio')], recent: [] };
  await loadSaved();
  const { app, input } = palette();
  type(input, 'zz');
  vi.advanceTimersByTime(120);
  await settle();
  // The row shows the shared tile.
  expect(target.querySelector('[aria-selected="true"] .tile .initials')?.textContent).toBe('ZR');
  key(input, 'Enter');
  await settle();
  expect(notice.text).toBe('zz Radio could not be played: The station is not reachable');
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
