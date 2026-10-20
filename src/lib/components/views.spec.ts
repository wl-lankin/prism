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
      return { albums: [album], artists: [artist], tracks: [track(2)] };
    case 'library_album_tracks':
      return [track(1), track(2), track(3)];
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
  await settle();
  const ranges = invoke.mock.calls.filter(([cmd]) => cmd === 'library_tracks_range');
  expect(ranges).toEqual([['library_tracks_range', { offset: 0, limit: 200 }]]);
  const rows = target.querySelectorAll('[role="option"]');
  expect(rows.length).toBeGreaterThan(0);
  expect(rows.length).toBeLessThan(60);
  expect(rows[0].textContent).toContain('Song 0');
  expect((target.querySelector('[role="listbox"]') as HTMLElement).style.height).toBe(`${30_000 * 44}px`);

  // A double-click plays the list from that row.
  rows[3].dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
  await settle();
  expect(invoke).toHaveBeenCalledWith('library_tracks_range', { offset: 0, limit: 1000 });
  expect(invoke).toHaveBeenCalledWith('player_play', expect.objectContaining({ startIndex: 3 }));
  unmount(app);
});
