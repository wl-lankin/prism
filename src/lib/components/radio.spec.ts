import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NearResult, Place, SavedStations, Station } from '../types';

const place: Place = {
  countryCode: 'DE',
  region: 'Testland',
  city: 'Testhausen',
  lat: 50.0,
  lon: 10.0,
  origin: 'detected',
  detectedAt: 1_700_000_000_000,
};

const station = (n: number, patch: Partial<Station> = {}): Station => ({
  id: `radio:00000000-0000-4000-8000-${String(n).padStart(12, '0')}`,
  name: `Test FM ${n}`,
  url: `http://127.0.0.1:1/${n}`,
  urlResolved: null,
  homepage: null,
  favicon: null,
  coverId: null,
  countryCode: 'DE',
  region: 'Testland',
  language: null,
  tags: ['pop', 'news'],
  codec: 'MP3',
  bitrateKbps: 128,
  hls: false,
  lastCheckOk: true,
  distanceKm: null,
  ...patch,
});

let nearResult: NearResult;
let savedResult: SavedStations;
let searchResult: Station[];
let playFails: string | null;

const invoke = vi.fn(async (cmd: string, _args?: Record<string, unknown>): Promise<unknown> => {
  switch (cmd) {
    case 'radio_near':
      return nearResult;
    case 'radio_saved':
      return savedResult;
    case 'radio_search':
      return searchResult;
    case 'radio_countries':
      return [
        { code: 'AT', name: 'Austria', stationCount: 40 },
        { code: 'DE', name: 'Germany', stationCount: 900 },
      ];
    case 'radio_regions':
      return [{ name: 'Testland', stationCount: 12 }];
    case 'radio_tags':
      return [
        { name: 'jazz', stationCount: 300 },
        { name: 'pop', stationCount: 500 },
      ];
    case 'radio_play':
      if (playFails) throw playFails;
      return undefined;
    case 'radio_favorite':
      return true;
    default:
      return undefined;
  }
});
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
const openUrl = vi.fn(async (_url: string) => {});
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: (url: string) => openUrl(url) }));

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= function () {};

const { default: RadioView } = await import('./RadioView.svelte');
const { default: Sidebar } = await import('./Sidebar.svelte');
const { default: StationList } = await import('./StationList.svelte');
const { default: StationTile, initials } = await import('./StationTile.svelte');
const { isWebUrl } = await import('./StationMenu.svelte');
const { nav } = await import('../stores/nav.svelte');
const { radio, facets } = await import('../stores/radio.svelte');
const { notice } = await import('../stores/notice.svelte');
const { favorites } = await import('../stores/favorites.svelte');
const { setLang } = await import('../i18n');

let target: HTMLElement;

beforeEach(() => {
  setLang('en');
  invoke.mockClear();
  openUrl.mockClear();
  playFails = null;
  nearResult = { stations: [station(1, { distanceKm: 12.4 }), station(2)], place, cached: false, stale: false, error: null };
  savedResult = { favorites: [station(3)], recent: [station(4)] };
  searchResult = [station(5)];
  radio.place = null;
  radio.near = { stations: [], loading: false, cached: false, stale: false, error: null };
  radio.search = { query: {}, stations: [], loading: false, error: null, more: false };
  radio.saved = { favorites: [], recent: [] };
  radio.nowPlaying = null;
  facets.countries = null;
  facets.regions = [];
  facets.regionsFor = null;
  facets.tags = null;
  notice.text = null;
  favorites.stations.clear();
  nav.view = 'radio';
  nav.param = 'near';
  nav.source = undefined;
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

const calls = (command: string) => invoke.mock.calls.filter(([cmd]) => cmd === command);
const rows = () => [...target.querySelectorAll<HTMLElement>('[role="option"]')];
const byLabel = (label: string) => target.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;

function click(el: Element) {
  el.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  flushSync();
}

async function choose(combobox: string, option: string) {
  click(byLabel(combobox));
  await settle();
  const item = [...target.querySelectorAll<HTMLElement>('[role="listbox"] [role="option"]')].find(
    (li) => li.textContent?.trim() === option,
  );
  expect(item, `option ${option}`).toBeTruthy();
  click(item!);
  await settle();
}

it('sidebar_radio_entry_opens_near', () => {
  nav.view = 'home';
  nav.param = undefined;
  const app = mount(Sidebar, { target });
  flushSync();
  const entry = [...target.querySelectorAll<HTMLButtonElement>('button.item')].find(
    (button) => button.textContent?.trim() === 'Radio',
  )!;
  expect(entry).toBeTruthy();
  expect(entry.getAttribute('aria-disabled')).toBeNull();
  expect(entry.textContent).not.toContain('Coming soon');
  click(entry);
  expect(nav.view).toBe('radio');
  expect(nav.param).toBe('near');
  expect(entry.getAttribute('aria-current')).toBe('page');
  // Every radio tab keeps it lit.
  nav.param = 'saved';
  flushSync();
  expect(entry.getAttribute('aria-current')).toBe('page');
  unmount(app);
});

it('near_lists_stations_with_distance', async () => {
  const app = mount(RadioView, { target });
  await settle();
  expect(calls('radio_near')).toEqual([['radio_near', { refresh: false }]]);
  expect(target.querySelector('.viewhead .meta')?.textContent).toBe('Near Testhausen, Germany');
  const list = rows();
  expect(list).toHaveLength(2);
  expect(list[0].textContent).toContain('Test FM 1');
  expect(list[0].textContent).toContain('12 km');
  expect(list[0].textContent).toContain('MP3 128');
  expect(list[0].textContent).toContain('Testland');
  expect(list[1].textContent).not.toContain('km');
  unmount(app);
});

it('near_shows_cached_hint', async () => {
  nearResult = { ...nearResult, cached: true, stale: true, error: 'unreachable' };
  const app = mount(RadioView, { target });
  await settle();
  expect(target.querySelector('.hint')?.textContent).toContain('cannot be reached');
  expect(rows()).toHaveLength(2);
  unmount(app);

  // A fresh list shows no hint.
  nearResult = { ...nearResult, cached: false, stale: false, error: null };
  const again = mount(RadioView, { target });
  await settle();
  expect(target.querySelector('.hint')).toBeNull();
  unmount(again);
});

it('near_empty_state_explains_and_opens_settings', async () => {
  nearResult = { stations: [], place, cached: false, stale: false, error: null };
  const app = mount(RadioView, { target });
  await settle();
  expect(target.querySelector('.empty .lead')?.textContent).toContain('Settings > Radio');
  click(target.querySelector('.empty button')!);
  expect(nav.view).toBe('settings');
  unmount(app);
});

describe('search tab', () => {
  beforeEach(() => {
    nav.param = 'search';
  });

  it('search_filters_reach_ipc', async () => {
    vi.useFakeTimers();
    const app = mount(RadioView, { target });
    await settle();
    expect(calls('radio_countries')).toHaveLength(1);
    expect(calls('radio_tags')).toHaveLength(1);

    const field = target.querySelector<HTMLInputElement>('input[type="search"]')!;
    field.value = 'jazz';
    field.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    vi.advanceTimersByTime(299);
    expect(calls('radio_search')).toHaveLength(0);
    vi.advanceTimersByTime(1);
    await settle();
    expect(calls('radio_search')).toEqual([['radio_search', { query: { name: 'jazz', offset: 0, limit: 100 } }]]);
    expect(rows()).toHaveLength(1);
    expect(rows()[0].textContent).toContain('Test FM 5');

    await choose('Country', 'Germany');
    vi.advanceTimersByTime(300);
    await settle();
    expect(calls('radio_regions')).toEqual([['radio_regions', { countryCode: 'DE' }]]);
    await choose('Region', 'Testland');
    await choose('Genre', 'jazz');
    vi.advanceTimersByTime(300);
    await settle();
    expect(calls('radio_search').at(-1)).toEqual([
      'radio_search',
      { query: { name: 'jazz', countryCode: 'DE', region: 'Testland', tag: 'jazz', offset: 0, limit: 100 } },
    ]);
    unmount(app);
  });

  it('region_select_waits_for_country', async () => {
    vi.useFakeTimers();
    const app = mount(RadioView, { target });
    await settle();
    expect(byLabel('Region').disabled).toBe(true);
    expect(calls('radio_regions')).toHaveLength(0);
    await choose('Country', 'Germany');
    expect(byLabel('Region').disabled).toBe(false);
    expect(facets.regions.map((r) => r.name)).toEqual(['Testland']);
    // Back to "Anywhere": the region closes again and its choice is gone.
    await choose('Country', 'Anywhere');
    expect(byLabel('Region').disabled).toBe(true);
    unmount(app);
  });

  it('search_says_so_when_the_directory_is_down', async () => {
    vi.useFakeTimers();
    const impl = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (cmd, args) => {
      if (cmd === 'radio_search') throw { code: 'unreachable' };
      return impl(cmd, args);
    });
    try {
      const app = mount(RadioView, { target });
      await settle();
      const field = target.querySelector<HTMLInputElement>('input[type="search"]')!;
      field.value = 'jazz';
      field.dispatchEvent(new Event('input', { bubbles: true }));
      flushSync();
      vi.advanceTimersByTime(300);
      await settle();
      expect(target.querySelector('.note.error')?.textContent).toBe('The radio directory cannot be reached right now.');
      unmount(app);
    } finally {
      invoke.mockImplementation(impl);
    }
  });

  it('show_more_asks_for_the_next_page', async () => {
    vi.useFakeTimers();
    searchResult = Array.from({ length: 100 }, (_, i) => station(i + 10));
    const app = mount(RadioView, { target });
    await settle();
    const field = target.querySelector<HTMLInputElement>('input[type="search"]')!;
    field.value = 'fm';
    field.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    vi.advanceTimersByTime(300);
    await settle();
    const more = [...target.querySelectorAll<HTMLButtonElement>('.more button')][0];
    expect(more.textContent?.trim()).toBe('Show more');
    searchResult = [station(500)];
    click(more);
    await settle();
    expect(calls('radio_search').at(-1)).toEqual(['radio_search', { query: { name: 'fm', offset: 100, limit: 100 } }]);
    expect(target.querySelector('.more')).toBeNull();
    unmount(app);
  });
});

it('saved_tab_lists_favorites_and_recent', async () => {
  nav.param = 'saved';
  const app = mount(RadioView, { target });
  await settle();
  expect([...target.querySelectorAll('h2')].map((h) => h.textContent)).toEqual(['Favorites', 'Recently played']);
  const names = rows().map((row) => row.querySelector('.name')?.textContent);
  expect(names).toEqual(['Test FM 3', 'Test FM 4']);
  unmount(app);

  savedResult = { favorites: [], recent: [] };
  radio.saved = { favorites: [], recent: [] };
  const empty = mount(RadioView, { target });
  await settle();
  expect(target.textContent).toContain('Stations you favorite or play show up here.');
  unmount(empty);
});

it('tabs_switch_through_the_segmented_control', async () => {
  const app = mount(RadioView, { target });
  await settle();
  click(target.querySelector('[data-tab="saved"]')!);
  expect(nav.view).toBe('radio');
  expect(nav.param).toBe('saved');
  unmount(app);
});

describe('station list', () => {
  it('row_play_plays_the_station', async () => {
    const app = mount(StationList, { target, props: { stations: [station(1), station(2)] } });
    await settle();
    click(byLabel('Play Test FM 2'));
    await settle();
    expect(calls('radio_play')).toHaveLength(1);
    expect((calls('radio_play')[0][1] as { station: Station }).station.id).toBe(station(2).id);

    // Enter on the selected row and a double click play as well.
    const list = target.querySelector<HTMLElement>('[role="listbox"]')!;
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await settle();
    expect((calls('radio_play')[1][1] as { station: Station }).station.id).toBe(station(1).id);
    rows()[1].dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    await settle();
    expect(calls('radio_play')).toHaveLength(3);
    unmount(app);
  });

  it('a_station_that_cannot_play_shows_a_notice', async () => {
    playFails = 'This station is not sending audio';
    const app = mount(StationList, { target, props: { stations: [station(1)] } });
    await settle();
    click(byLabel('Play Test FM 1'));
    await settle();
    expect(notice.text).toBe('Test FM 1 could not be played: This station is not sending audio');
    unmount(app);
  });

  it('the_heart_saves_the_station_itself', async () => {
    const app = mount(StationList, { target, props: { stations: [station(1)] } });
    await settle();
    click(byLabel('Favorite: Test FM 1'));
    await settle();
    expect(calls('favorites_set')).toHaveLength(0);
    expect(calls('radio_favorite')).toEqual([['radio_favorite', { station: station(1), on: true }]]);
    unmount(app);
  });

  it('only_the_rows_near_the_viewport_exist', async () => {
    const many = Array.from({ length: 5000 }, (_, i) => station(i + 1));
    const app = mount(StationList, { target, props: { stations: many } });
    await settle();
    expect(rows().length).toBeGreaterThan(0);
    expect(rows().length).toBeLessThan(60);
    expect(rows()[0].getAttribute('aria-setsize')).toBe('5000');
    unmount(app);

    const all = mount(StationList, { target, props: { stations: many.slice(0, 120), virtual: false } });
    await settle();
    expect(rows()).toHaveLength(120);
    unmount(all);
  });

  it('homepage_menu_only_for_http_links', async () => {
    const stations = [
      station(1, { homepage: 'https://radio.example.test/' }),
      station(2, { homepage: 'file:///etc/hosts' }),
      station(3, { homepage: 'javascript:alert(1)' }),
      station(4, { homepage: null }),
    ];
    const app = mount(StationList, { target, props: { stations } });
    await settle();
    const menuItems = () => [...document.querySelectorAll('[role="menu"] [role="menuitem"]')].map((el) => el.textContent?.trim());

    rows()[0].dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 40, clientY: 40 }));
    await settle();
    expect(menuItems()).toEqual(['Play', 'Add to favorites', 'Open homepage']);
    click([...document.querySelectorAll<HTMLElement>('[role="menuitem"]')][2]);
    expect(openUrl).toHaveBeenCalledWith('https://radio.example.test/');
    await settle();
    expect(document.querySelector('[role="menu"]')).toBeNull();

    for (const index of [1, 2, 3]) {
      rows()[index].dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 40, clientY: 40 }));
      await settle();
      expect(menuItems()).toEqual(['Play', 'Add to favorites']);
      document.querySelector('[role="menu"]')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
      await settle();
    }
    expect(openUrl).toHaveBeenCalledTimes(1);

    expect(isWebUrl('http://a.test')).toBe(true);
    expect(isWebUrl('ftp://a.test')).toBe(false);
    expect(isWebUrl('not a url')).toBe(false);
    expect(isWebUrl(null)).toBe(false);
    unmount(app);
  });
});

describe('station tile', () => {
  it('tile_falls_back_to_initials', async () => {
    const without = mount(StationTile, { target, props: { name: 'Test FM 1', coverId: null } });
    flushSync();
    expect(target.querySelector('.initials')?.textContent).toBe('TF');
    expect(target.querySelector('img')).toBeNull();
    unmount(without);

    // A logo that fails to load leaves the initials.
    const broken = mount(StationTile, { target, props: { name: 'Antenne', coverId: 'rb-0123456789abcdef0123' } });
    flushSync();
    const img = target.querySelector('img')!;
    expect(img.getAttribute('src')).toContain('/cover/rb-0123456789abcdef0123/96');
    img.dispatchEvent(new Event('error'));
    await settle();
    expect(target.querySelector('img')).toBeNull();
    expect(target.querySelector('.initials')?.textContent).toBe('AN');
    unmount(broken);

    // A logo that loads takes the initials' place.
    const good = mount(StationTile, { target, props: { name: 'Antenne', coverId: 'rb-aaaaaaaaaaaaaaaaaaaa' } });
    flushSync();
    target.querySelector('img')!.dispatchEvent(new Event('load'));
    await settle();
    expect(target.querySelector('.initials')).toBeNull();
    expect(target.querySelector('.tile')?.classList.contains('logo')).toBe(true);
    unmount(good);
  });

  it('initials_take_two_letters', () => {
    expect(initials('Radio Testhausen')).toBe('RT');
    expect(initials('jazz')).toBe('JA');
    expect(initials('  *** 1LIVE ***')).toBe('1L');
    expect(initials('...')).toBe('?');
  });
});
