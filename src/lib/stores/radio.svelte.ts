import { lang, t } from '../i18n';
import {
  onFavoritesChanged,
  onRadioNowPlaying,
  radioFavorite,
  radioNear,
  radioCountries,
  radioNowPlaying,
  radioOpenHomepage,
  radioPlace,
  radioPlay,
  radioRedetect,
  radioRegions,
  radioSaved,
  radioSearch,
  radioTags,
} from '../ipc';
import type {
  NowPlaying,
  Place,
  RadioCountry,
  RadioErrorCode,
  RadioRegion,
  RadioTag,
  Station,
  StationQueryArgs,
} from '../types';
import { changeFavorite, favorites } from './favorites.svelte';
import { notify } from './notice.svelte';

/** Results per search page ("Show more" asks for the next one). */
export const SEARCH_PAGE = 100;

/** How long the search field rests before the directory is asked. */
export const SEARCH_DEBOUNCE_MS = 300;

/** Radio as the views show it. `near` and `search` keep their last good list while a newer
 *  call is on its way or failed; `error` is a code from the backend (`unreachable`,
 *  `server`, `badResponse`) or `internal`. */
export const radio = $state({
  place: null as Place | null,
  near: {
    stations: [] as Station[],
    loading: false,
    cached: false,
    stale: false,
    error: null as RadioErrorCode | null,
  },
  search: {
    query: {} as StationQueryArgs,
    stations: [] as Station[],
    loading: false,
    error: null as RadioErrorCode | null,
    /** The last page was full, so another one may exist. */
    more: false,
  },
  saved: { favorites: [] as Station[], recent: [] as Station[] },
  nowPlaying: null as NowPlaying | null,
});

const CODES: readonly RadioErrorCode[] = ['unreachable', 'server', 'badResponse', 'internal'];

function errorCode(err: unknown): RadioErrorCode {
  const code = (err as { code?: unknown } | null)?.code;
  return CODES.find((known) => known === code) ?? 'internal';
}

function errorText(err: unknown): string {
  if (typeof err === 'string') return err;
  return err instanceof Error ? err.message : String(err);
}

let nearRun = 0;

/** Loads "near you". The cache answers unless `refresh`. A failed or empty-handed answer
 *  keeps the list on screen and only sets `error`. */
export async function loadNear(refresh = false): Promise<void> {
  const run = ++nearRun;
  radio.near.loading = true;
  try {
    const result = await radioNear(refresh);
    if (run !== nearRun) return;
    radio.place = result.place;
    // The backend falls back to its cache; an empty answer with an error must not blank a list.
    if (!(result.error && result.stations.length === 0 && radio.near.stations.length > 0)) {
      radio.near.stations = result.stations;
    }
    radio.near.cached = result.cached;
    radio.near.stale = result.stale;
    radio.near.error = result.error;
  } catch (err) {
    if (run !== nearRun) return;
    console.error('loading the stations near you failed', err);
    radio.near.error = 'internal';
  } finally {
    if (run === nearRun) radio.near.loading = false;
  }
}

/** The place "near you" uses (looks it up on first use in automatic mode). */
export async function loadPlace(): Promise<void> {
  try {
    radio.place = await radioPlace();
  } catch (err) {
    console.error('loading the radio place failed', err);
  }
}

/** Asks the location service again (automatic mode) and reloads the near list for it. */
export async function redetectPlace(): Promise<void> {
  try {
    radio.place = await radioRedetect();
  } catch (err) {
    console.error('detecting the place failed', err);
    return;
  }
  await loadNear(true);
}

let searchRun = 0;
let searchTimer: ReturnType<typeof setTimeout> | undefined;
let searchPending = false;

const hasFilter = (query: StationQueryArgs) =>
  Boolean(query.name?.trim() || query.countryCode || query.region || query.tag);

async function fetchSearch(run: number, query: StationQueryArgs, offset: number): Promise<void> {
  radio.search.loading = true;
  try {
    const found = await radioSearch({ ...query, offset, limit: SEARCH_PAGE });
    if (run !== searchRun) return;
    radio.search.stations = offset === 0 ? found : [...radio.search.stations, ...found];
    radio.search.more = found.length >= SEARCH_PAGE;
    radio.search.error = null;
  } catch (err) {
    if (run !== searchRun) return;
    radio.search.error = errorCode(err);
    if (offset === 0) {
      radio.search.stations = [];
      radio.search.more = false;
    }
  } finally {
    if (run === searchRun) radio.search.loading = false;
  }
}

/** Searches the directory after the field rested for 300 ms; the latest query wins and an
 *  older answer that arrives late is dropped. A query without any filter clears the list. */
export function runSearch(query: StationQueryArgs): void {
  clearTimeout(searchTimer);
  const run = ++searchRun;
  radio.search.query = query;
  // The old query's "more" must not page into the new one while the debounce waits.
  radio.search.more = false;
  searchPending = true;
  if (!hasFilter(query)) {
    searchPending = false;
    radio.search.stations = [];
    radio.search.error = null;
    radio.search.loading = false;
    return;
  }
  searchTimer = setTimeout(() => {
    searchPending = false;
    void fetchSearch(run, query, 0);
  }, SEARCH_DEBOUNCE_MS);
}

/** Appends the next page of the current search. */
export async function searchMore(): Promise<void> {
  if (searchPending || !radio.search.more || radio.search.loading) return;
  await fetchSearch(++searchRun, radio.search.query, radio.search.stations.length);
}

export async function loadSaved(): Promise<void> {
  try {
    const saved = await radioSaved();
    radio.saved.favorites = saved.favorites;
    radio.saved.recent = saved.recent;
  } catch (err) {
    console.error('loading the saved stations failed', err);
  }
}

/** Plays the station as a queue of one. Rejects with the backend's message when it cannot
 *  be played; the caller shows it. */
export function playStation(station: Station): Promise<void> {
  return radioPlay(station);
}

/** Flips a station's heart: shown at once and saved through `radio_favorite`; the saved
 *  lists reload on `favorites://changed`. A failure puts the old state back with a notice. */
export async function toggleStationFavorite(station: Station): Promise<void> {
  const on = !favorites.stations.has(station.id);
  try {
    await changeFavorite('station', station.id, on, () => radioFavorite(station, on));
  } catch (err) {
    notify(t('favorites.failed', { message: errorText(err) }));
  }
  // The saved lists reload on the `favorites://changed` event the backend sends.
}

/** A station the app already holds (saved, recent, near you or a search result). */
export function findStation(id: string): Station | null {
  const lists = [radio.saved.favorites, radio.saved.recent, radio.near.stations, radio.search.stations];
  for (const list of lists) {
    const found = list.find((station) => station.id === id);
    if (found) return found;
  }
  return null;
}

let ensured: string | null = null;

/** A played station is stored, so the saved lists know it; a window that has not read them
 *  since (the mini player) reads them once per station so its heart has a station to save. */
export function ensureStation(id: string): void {
  if (findStation(id)) {
    if (ensured === id) ensured = null;
    return;
  }
  if (ensured === id) return;
  ensured = id;
  void loadSaved();
}

/** A one-off name search for the search palette. It leaves `radio.search` alone, which
 *  belongs to the Radio view. Rejects like `radioSearch` when the directory cannot answer. */
export function findStations(name: string, limit: number): Promise<Station[]> {
  return radioSearch({ name, limit });
}

/** The countries the directory knows, by name. Rejects when the directory cannot answer. */
export async function listCountries(): Promise<RadioCountry[]> {
  const countries = await radioCountries();
  return [...(countries ?? [])].sort((a, b) => a.name.localeCompare(b.name));
}

/** The regions of one country, by name. Rejects when the directory cannot answer. */
export async function listRegions(countryCode: string): Promise<RadioRegion[]> {
  const regions = await radioRegions(countryCode);
  return [...(regions ?? [])].sort((a, b) => a.name.localeCompare(b.name));
}

/** The country's name in the UI language ("DE" -> "Germany"); the code when it is unknown. */
export function countryName(code: string): string {
  try {
    return new Intl.DisplayNames([lang()], { type: 'region' }).of(code.toUpperCase()) ?? code;
  } catch {
    return code;
  }
}

/** Plays a station and, when the backend refuses, says so in a notice. */
export async function startStation(station: Station): Promise<void> {
  try {
    await playStation(station);
  } catch (err) {
    notify(t('radio.play.failed', { name: station.name, message: errorText(err) }));
  }
}

/** Opens the station's homepage through the backend, which only opens http(s) links. */
export async function openHomepage(station: Station): Promise<void> {
  try {
    await radioOpenHomepage(station.id);
  } catch (err) {
    console.error('opening the homepage failed', err);
  }
}

/** The lists behind the search filters. Each loads once and is kept; `null` is not loaded. */
export const facets = $state({
  countries: null as RadioCountry[] | null,
  regions: [] as RadioRegion[],
  regionsFor: null as string | null,
  tags: null as RadioTag[] | null,
});

/** How many genres the filter offers. */
export const TOP_TAGS = 40;

export async function loadCountries(): Promise<void> {
  if (facets.countries) return;
  try {
    facets.countries = await radioCountries();
  } catch (err) {
    console.error('loading the radio countries failed', err);
  }
}

export async function loadTags(): Promise<void> {
  if (facets.tags) return;
  try {
    const tags = await radioTags();
    facets.tags = [...tags].sort((a, b) => b.stationCount - a.stationCount).slice(0, TOP_TAGS);
  } catch (err) {
    console.error('loading the radio genres failed', err);
  }
}

/** Loads the regions of a country; none for an empty code. The latest country wins. */
export async function loadRegions(countryCode: string): Promise<void> {
  facets.regionsFor = countryCode || null;
  facets.regions = [];
  if (!countryCode) return;
  try {
    const regions = await radioRegions(countryCode);
    if (facets.regionsFor === countryCode) facets.regions = regions;
  } catch (err) {
    console.error('loading the radio regions failed', err);
  }
}

/** The stream title, only while `track` is the station it belongs to. */
export function currentTitle(track: { id: string } | null | undefined): string | null {
  const now = radio.nowPlaying;
  return track && now && track.id === now.stationId ? now.title : null;
}

/** Follows `radio://now-playing` and `favorites://changed`, and loads the current pair for a
 *  window that opens late. Resolves with the cleanup. */
export async function initRadio(): Promise<() => void> {
  const stops: Array<() => void> = [];
  let heard = false;
  try {
    stops.push(
      await onRadioNowPlaying((now) => {
        heard = true;
        radio.nowPlaying = now;
      }),
    );
  } catch (err) {
    console.error('following the radio title failed', err);
  }
  try {
    stops.push(await onFavoritesChanged(() => void loadSaved()));
  } catch (err) {
    console.error('following the station favorites failed', err);
  }
  try {
    const now = await radioNowPlaying();
    // An event that arrived meanwhile is newer than this answer.
    if (now && !heard) radio.nowPlaying = now;
  } catch (err) {
    console.error('loading the current station failed', err);
  }
  // Home, Favorites and the search palette read the saved lists without loading them.
  void loadSaved();
  return () => stops.forEach((stop) => stop());
}
