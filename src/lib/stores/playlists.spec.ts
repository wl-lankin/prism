import { beforeEach, expect, it, vi } from 'vitest';
import type { PlaylistEntry, Track } from '../types';

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

const { playPlaylist } = await import('../play');
const { initPlaylists, playlists, suggestName } = await import('./playlists.svelte');
const { notice } = await import('./notice.svelte');
const { setLang } = await import('../i18n');

const track = (id: string): Track =>
  ({ id, title: id, artist: 'A', albumArtist: 'A', album: 'B', albumId: 'b', durationMs: 1000 }) as Track;

/** Entries 0..n; `null` marks one whose track is not in the index. */
function entries(...ids: (string | null)[]): PlaylistEntry[] {
  return ids.map((id, position) => ({
    position,
    trackId: id ?? `local:gone${position}`,
    track: id ? track(id) : null,
  }));
}

beforeEach(() => {
  setLang('en');
  listeners.clear();
  invoke.mockReset();
  notice.text = null;
  playlists.list = null;
});

it('plays a playlist from the start, skipping entries without a track', async () => {
  invoke.mockImplementation(async (cmd: string) => (cmd === 'playlist_entries' ? entries('t1', null, 't3') : undefined));
  await playPlaylist(4);
  expect(invoke).toHaveBeenCalledWith('playlist_entries', { id: 4 });
  expect(invoke).toHaveBeenCalledWith('player_play', { trackIds: ['t1', 't3'], startIndex: 0 });
});

it('starts at the entry for the given position among the playable ones', async () => {
  invoke.mockImplementation(async (cmd: string) =>
    cmd === 'playlist_entries' ? entries('t1', null, 't3', 't4') : undefined,
  );
  await playPlaylist(4, 3);
  expect(invoke).toHaveBeenCalledWith('player_play', { trackIds: ['t1', 't3', 't4'], startIndex: 2 });
});

it('starts at the next playable entry when the chosen one is missing', async () => {
  invoke.mockImplementation(async (cmd: string) =>
    cmd === 'playlist_entries' ? entries('t1', null, 't3', 't4') : undefined,
  );
  await playPlaylist(4, 1);
  expect(invoke).toHaveBeenCalledWith('player_play', { trackIds: ['t1', 't3', 't4'], startIndex: 1 });
});

it('plays nothing when no playable entry comes at or after the given position', async () => {
  invoke.mockImplementation(async (cmd: string) => (cmd === 'playlist_entries' ? entries('t1', 't2', null) : undefined));
  await playPlaylist(4, 2);
  expect(invoke).not.toHaveBeenCalledWith('player_play', expect.anything());
});

it('plays nothing and says so when no entry has a track', async () => {
  invoke.mockImplementation(async (cmd: string) => (cmd === 'playlist_entries' ? entries(null, null) : undefined));
  await playPlaylist(4);
  expect(invoke).not.toHaveBeenCalledWith('player_play', expect.anything());
  expect(notice.text).toBe('None of the songs in this playlist are in your library.');
});

it('loads the playlists on init and reloads them on playlists://changed', async () => {
  const first = [{ id: 1, name: 'Road trip', trackCount: 0, durationMs: 0, coverIds: [], updatedAt: 1 }];
  const second = [...first, { id: 2, name: 'Late', trackCount: 0, durationMs: 0, coverIds: [], updatedAt: 2 }];
  const remote = [
    {
      id: 'navidrome:p1',
      name: 'Server mix',
      owner: 'demo',
      trackCount: 3,
      durationMs: 600_000,
      coverIds: [],
      updatedAt: 5,
    },
  ];
  let local = first;
  let server: typeof remote = [];
  invoke.mockImplementation(async (cmd: string) =>
    cmd === 'playlists_list' ? local : cmd === 'navidrome_playlists' ? server : undefined,
  );
  const stop = await initPlaylists();
  expect(invoke).toHaveBeenCalledWith('playlists_list', undefined);
  expect(invoke).toHaveBeenCalledWith('navidrome_playlists', undefined);
  expect(playlists.list?.map((p) => p.name)).toEqual(['Road trip']);
  expect(playlists.remote).toEqual([]);
  const version = playlists.version;
  // A Navidrome sync that changed the server playlists sends the same event.
  local = second;
  server = remote;
  listeners.get('playlists://changed')?.({ event: 'playlists://changed', id: 0, payload: null });
  await vi.waitFor(() => expect(playlists.list?.length).toBe(2));
  expect(playlists.remote?.map((p) => p.name)).toEqual(['Server mix']);
  expect(playlists.version).toBeGreaterThan(version);
  stop();
});

it('keeps the Prism playlists when the server playlists fail to load', async () => {
  const first = [{ id: 1, name: 'Road trip', trackCount: 0, durationMs: 0, coverIds: [], updatedAt: 1 }];
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'playlists_list') return first;
    if (cmd === 'navidrome_playlists') throw new Error('database is locked');
    return undefined;
  });
  playlists.remote = null;
  const { reloadPlaylists } = await import('./playlists.svelte');
  await reloadPlaylists();
  expect(playlists.list?.map((p) => p.name)).toEqual(['Road trip']);
  expect(playlists.remote).toEqual([]);
  expect(notice.text).toBeNull();
});

it('suggests a name that is not taken yet', () => {
  playlists.list = [];
  expect(suggestName()).toBe('New playlist');
  playlists.list = [
    { id: 1, name: 'New playlist', trackCount: 0, durationMs: 0, coverIds: [], updatedAt: 1 },
    { id: 2, name: 'New playlist 2', trackCount: 0, durationMs: 0, coverIds: [], updatedAt: 1 },
  ];
  expect(suggestName()).toBe('New playlist 3');
});
