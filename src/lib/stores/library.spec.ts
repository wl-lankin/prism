import { beforeEach, expect, it, vi } from 'vitest';
import type { Album, Artist } from '../types';

type Handler = (event: { event: string; id: number; payload: unknown }) => void;

const listeners = new Map<string, Handler>();

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (event: string, handler: Handler) => {
    listeners.set(event, handler);
    return () => {};
  }),
}));

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const { library, initLibrary, reloadLibrary, albumsOf, artistsOf } = await import('./library.svelte');

const progress = (done: number, total: number) =>
  listeners.get('library://scan-progress')!({
    event: 'library://scan-progress',
    id: 1,
    payload: { folder: '/music', done, total },
  });

beforeEach(() => {
  listeners.clear();
  invoke.mockReset();
  invoke.mockResolvedValue([]);
  library.scan = null;
});

it('shows a running scan and clears it for an empty 0 of 0 progress at once', async () => {
  const stop = initLibrary();
  await vi.waitFor(() => expect(listeners.has('library://scan-progress')).toBe(true));

  progress(3, 10);
  expect(library.scan).toEqual({ folder: '/music', done: 3, total: 10 });

  progress(0, 0);
  expect(library.scan).toBeNull();
  stop();
});

const album = (id: string, source: Album['source']): Album => ({
  id,
  source,
  title: id,
  artist: 'A',
  year: null,
  coverId: null,
  trackCount: 1,
  durationMs: 1000,
});

const artist = (id: string, source: Artist['source']): Artist => ({
  id,
  source,
  name: 'A',
  albumCount: 1,
  trackCount: 1,
  coverId: null,
});

it('fills the track counts from one call per source, all being their sum', async () => {
  invoke.mockImplementation(async (cmd: string, args?: { source?: string }) => {
    if (cmd !== 'library_track_count') return [];
    if (args?.source === 'local') return 7;
    if (args?.source === 'navidrome') return 5;
    return 999;
  });
  await reloadLibrary();
  expect(library.trackCounts).toEqual({ all: 12, local: 7, navidrome: 5 });
  const counts = invoke.mock.calls.filter(([cmd]) => cmd === 'library_track_count').map(([, args]) => args);
  expect(counts).toEqual([{ source: 'local' }, { source: 'navidrome' }]);
});

it('filters albums and artists by source', async () => {
  const albums = [album('local:1', 'local'), album('navidrome:2', 'navidrome'), album('local:3', 'local')];
  const artists = [artist('A', 'local'), artist('navidrome:a', 'navidrome')];
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'library_albums') return albums;
    if (cmd === 'library_artists') return artists;
    return 0;
  });
  await reloadLibrary();
  expect(albumsOf('navidrome').map((a) => a.id)).toEqual(['navidrome:2']);
  expect(albumsOf('local').map((a) => a.id)).toEqual(['local:1', 'local:3']);
  expect(albumsOf()).toBe(library.albums);
  expect(artistsOf('navidrome').map((a) => a.id)).toEqual(['navidrome:a']);
  expect(artistsOf()).toBe(library.artists);
});

it('gives empty lists before the first load', () => {
  library.albums = null;
  library.artists = null;
  expect(albumsOf('local')).toEqual([]);
  expect(artistsOf()).toEqual([]);
});
