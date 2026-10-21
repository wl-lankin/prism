import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { NearResult, Place, Station } from '../types';

type Handler = (event: { event: string; id: number; payload: unknown }) => void;

const listeners = new Map<string, Handler>();

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (event: string, handler: Handler) => {
    listeners.set(event, handler);
    return () => listeners.delete(event);
  }),
}));

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const radioModule = await import('./radio.svelte');
const { radio, initRadio, loadNear, runSearch, searchMore, loadSaved, playStation, toggleStationFavorite, currentTitle } =
  radioModule;
const { favorites } = await import('./favorites.svelte');
const { notice } = await import('./notice.svelte');

const place: Place = {
  countryCode: 'DE',
  region: 'Testland',
  city: 'Testhausen',
  lat: 50.0,
  lon: 10.0,
  origin: 'detected',
  detectedAt: 1_700_000_000_000,
};

const station = (n: number): Station => ({
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
  tags: [],
  codec: 'MP3',
  bitrateKbps: 128,
  hls: false,
  lastCheckOk: true,
  distanceKm: null,
});

const near = (patch: Partial<NearResult>): NearResult => ({
  stations: [station(1), station(2)],
  place,
  cached: false,
  stale: false,
  error: null,
  ...patch,
});

/** Calls to one command, in order. */
const calls = (command: string) => invoke.mock.calls.filter((call) => call[0] === command);

beforeEach(() => {
  vi.useFakeTimers();
  listeners.clear();
  invoke.mockReset();
  radio.place = null;
  radio.near = { stations: [], loading: false, cached: false, stale: false, error: null };
  radio.search = { query: {}, stations: [], loading: false, error: null, more: false };
  radio.saved = { favorites: [], recent: [] };
  radio.nowPlaying = null;
  favorites.stations.clear();
  notice.text = null;
});

afterEach(() => {
  vi.useRealTimers();
});

it('search_is_debounced_and_latest_wins', async () => {
  let release!: (stations: Station[]) => void;
  const slow = new Promise<Station[]>((resolve) => (release = resolve));
  invoke.mockImplementation((command: string, args?: { query?: { name?: string } }) => {
    if (command !== 'radio_search') return Promise.resolve(undefined);
    return args?.query?.name === 'jazz' ? slow : Promise.resolve([station(9)]);
  });

  runSearch({ name: 'ja' });
  runSearch({ name: 'jazz' });
  expect(calls('radio_search')).toHaveLength(0);
  await vi.advanceTimersByTimeAsync(299);
  expect(calls('radio_search')).toHaveLength(0);
  await vi.advanceTimersByTimeAsync(1);
  expect(calls('radio_search')).toHaveLength(1);
  expect(calls('radio_search')[0][1]).toEqual({ query: { name: 'jazz', offset: 0, limit: 100 } });
  expect(radio.search.loading).toBe(true);

  // A newer query starts and answers before the older one does.
  runSearch({ name: 'rock' });
  await vi.advanceTimersByTimeAsync(300);
  expect(radio.search.stations.map((s) => s.id)).toEqual([station(9).id]);

  release([station(1), station(2)]);
  await vi.advanceTimersByTimeAsync(0);
  expect(radio.search.stations.map((s) => s.id)).toEqual([station(9).id]);
  expect(radio.search.loading).toBe(false);
});

it('an empty search clears the results without asking the directory', async () => {
  radio.search.stations = [station(1)];
  runSearch({ name: '  ' });
  await vi.advanceTimersByTimeAsync(300);
  expect(calls('radio_search')).toHaveLength(0);
  expect(radio.search.stations).toEqual([]);
});

it('a failed search keeps the code and an empty list', async () => {
  invoke.mockRejectedValue({ code: 'unreachable' });
  runSearch({ name: 'jazz' });
  await vi.advanceTimersByTimeAsync(300);
  expect(radio.search.error).toBe('unreachable');
  expect(radio.search.stations).toEqual([]);
  expect(radio.search.loading).toBe(false);
});

it('show more appends the next page', async () => {
  const page = (from: number) => Array.from({ length: 100 }, (_, i) => station(from + i));
  invoke.mockResolvedValueOnce(page(0)).mockResolvedValueOnce([station(500)]);
  runSearch({ name: 'jazz' });
  await vi.advanceTimersByTimeAsync(300);
  expect(radio.search.more).toBe(true);
  await searchMore();
  expect(calls('radio_search')[1][1]).toEqual({ query: { name: 'jazz', offset: 100, limit: 100 } });
  expect(radio.search.stations).toHaveLength(101);
  expect(radio.search.more).toBe(false);
});

it('title_shows_only_for_the_current_station', async () => {
  invoke.mockResolvedValue(null);
  const stop = await initRadio();
  listeners.get('radio://now-playing')!({
    event: 'radio://now-playing',
    id: 1,
    payload: { stationId: station(1).id, title: 'Artist - Song' },
  });
  expect(radio.nowPlaying).toEqual({ stationId: station(1).id, title: 'Artist - Song' });
  expect(currentTitle({ id: station(1).id })).toBe('Artist - Song');
  expect(currentTitle({ id: station(2).id })).toBeNull();
  expect(currentTitle({ id: 'local:t1' })).toBeNull();
  expect(currentTitle(null)).toBeNull();
  listeners.get('radio://now-playing')!({
    event: 'radio://now-playing',
    id: 2,
    payload: { stationId: station(1).id, title: null },
  });
  expect(currentTitle({ id: station(1).id })).toBeNull();
  stop();
});

it('init loads the pair for a window that opens later', async () => {
  invoke.mockImplementation((command: string) =>
    Promise.resolve(command === 'radio_now_playing' ? { stationId: station(3).id, title: 'Late' } : undefined),
  );
  const stop = await initRadio();
  expect(radio.nowPlaying?.title).toBe('Late');
  stop();
});

it('near_error_keeps_cached_list', async () => {
  invoke.mockResolvedValueOnce(near({ cached: true }));
  await loadNear();
  expect(radio.near.stations).toHaveLength(2);
  expect(radio.place).toEqual(place);

  // The directory is down and the backend has nothing newer: the list stays.
  invoke.mockResolvedValueOnce(near({ stations: [], cached: true, stale: true, error: 'unreachable' }));
  await loadNear(true);
  expect(calls('radio_near')[1][1]).toEqual({ refresh: true });
  expect(radio.near.stations).toHaveLength(2);
  expect(radio.near.error).toBe('unreachable');
  expect(radio.near.stale).toBe(true);
  expect(radio.near.loading).toBe(false);

  // The call itself failing changes nothing but the error.
  invoke.mockRejectedValueOnce('boom');
  await loadNear(true);
  expect(radio.near.stations).toHaveLength(2);
  expect(radio.near.error).toBe('internal');
});

it('a fresh near list replaces the old one', async () => {
  invoke.mockResolvedValueOnce(near({}));
  await loadNear();
  invoke.mockResolvedValueOnce(near({ stations: [station(7)] }));
  await loadNear(true);
  expect(radio.near.stations.map((s) => s.id)).toEqual([station(7).id]);
  expect(radio.near.error).toBeNull();
});

it('favorite_toggle_reloads_saved', async () => {
  const saved = { favorites: [station(1)], recent: [] };
  invoke.mockImplementation((command: string) => {
    if (command === 'radio_favorite') return Promise.resolve(true);
    if (command === 'radio_saved') return Promise.resolve(saved);
    return Promise.resolve(undefined);
  });
  await toggleStationFavorite(station(1));
  expect(calls('radio_favorite')[0][1]).toEqual({ station: station(1), on: true });
  expect(favorites.stations.has(station(1).id)).toBe(true);
  expect(calls('radio_saved')).toHaveLength(1);
  expect(radio.saved.favorites).toEqual(saved.favorites);

  // Off again.
  saved.favorites = [];
  await toggleStationFavorite(station(1));
  expect(calls('radio_favorite')[1][1]).toEqual({ station: station(1), on: false });
  expect(favorites.stations.has(station(1).id)).toBe(false);
  expect(radio.saved.favorites).toEqual([]);
});

it('a failed favorite goes back with a notice', async () => {
  invoke.mockRejectedValue('no space left');
  await toggleStationFavorite(station(1));
  expect(favorites.stations.has(station(1).id)).toBe(false);
  expect(notice.text).toContain('no space left');
});

it('playing sends the station and a failure reaches the caller', async () => {
  invoke.mockResolvedValueOnce(undefined);
  await playStation(station(1));
  expect(calls('radio_play')[0][1]).toEqual({ station: station(1) });
  invoke.mockRejectedValueOnce('this is not a playable station');
  await expect(playStation(station(2))).rejects.toBe('this is not a playable station');
});

it('the saved lists follow favorites changes', async () => {
  invoke.mockImplementation((command: string) =>
    Promise.resolve(command === 'radio_saved' ? { favorites: [station(4)], recent: [station(5)] } : null),
  );
  const stop = await initRadio();
  listeners.get('favorites://changed')!({ event: 'favorites://changed', id: 1, payload: null });
  await vi.advanceTimersByTimeAsync(0);
  expect(radio.saved.favorites.map((s) => s.id)).toEqual([station(4).id]);
  expect(radio.saved.recent.map((s) => s.id)).toEqual([station(5).id]);
  stop();
  expect(listeners.has('favorites://changed')).toBe(false);
  await loadSaved();
});
