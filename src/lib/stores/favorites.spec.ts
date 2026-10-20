import { beforeEach, expect, it, vi } from 'vitest';

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

const { favorites, initFavorites, isFavorite, setFavorite } = await import('./favorites.svelte');
const { notice } = await import('./notice.svelte');

/** A promise and the functions that settle it. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  listeners.clear();
  invoke.mockReset();
  favorites.tracks.clear();
  favorites.albums.clear();
  favorites.artists.clear();
  notice.text = null;
});

it('loads the ids on init', async () => {
  invoke.mockResolvedValue({ tracks: ['local:t1'], albums: ['a1'], artists: ['Lumen Field'] });
  const stop = await initFavorites();
  expect(invoke).toHaveBeenCalledWith('favorites_ids', undefined);
  expect(isFavorite('track', 'local:t1')).toBe(true);
  expect(isFavorite('album', 'a1')).toBe(true);
  expect(isFavorite('artist', 'Lumen Field')).toBe(true);
  expect(isFavorite('track', 'local:t2')).toBe(false);
  stop();
});

it('sets a favorite before the IPC call resolves', async () => {
  const call = deferred<boolean>();
  invoke.mockReturnValue(call.promise);
  const done = setFavorite('track', 'local:t1', true);
  expect(isFavorite('track', 'local:t1')).toBe(true);
  expect(invoke).toHaveBeenCalledWith('favorites_set', { kind: 'track', id: 'local:t1', on: true });
  call.resolve(true);
  await done;
  expect(isFavorite('track', 'local:t1')).toBe(true);
  expect(notice.text).toBeNull();
});

it('reverts the change with a notice when the IPC call fails', async () => {
  favorites.albums.add('a1');
  const call = deferred<boolean>();
  invoke.mockReturnValue(call.promise);
  const done = setFavorite('album', 'a1', false);
  expect(isFavorite('album', 'a1')).toBe(false);
  call.reject('database is locked');
  await done;
  expect(isFavorite('album', 'a1')).toBe(true);
  expect(notice.text).toContain('database is locked');
});

it('reloads the ids on favorites://changed', async () => {
  invoke.mockResolvedValue({ tracks: [], albums: [], artists: [] });
  const stop = await initFavorites();
  expect(isFavorite('artist', 'Lumen Field')).toBe(false);

  invoke.mockResolvedValue({ tracks: ['local:t9'], albums: [], artists: ['Lumen Field'] });
  listeners.get('favorites://changed')!({ event: 'favorites://changed', id: 1, payload: null });
  await vi.waitFor(() => expect(isFavorite('artist', 'Lumen Field')).toBe(true));
  expect(isFavorite('track', 'local:t9')).toBe(true);
  expect(invoke.mock.calls.filter(([cmd]) => cmd === 'favorites_ids')).toHaveLength(2);
  stop();
});

it('stops following the event after cleanup', async () => {
  invoke.mockResolvedValue({ tracks: [], albums: [], artists: [] });
  const stop = await initFavorites();
  stop();
  expect(listeners.has('favorites://changed')).toBe(false);
});
