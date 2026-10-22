import { beforeEach, expect, it, vi } from 'vitest';

const calls: [string, Record<string, unknown> | undefined][] = [];
const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
  calls.push([cmd, args]);
  if (cmd === 'settings_set') return { ...settings, ...(args as { patch: object }).patch };
  if (cmd === 'library_album_tracks') return ['t1', 't2', 't3', 't4'].map((id) => ({ id }));
  return undefined;
});
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

const { playAlbum, playShuffled } = await import('./play');
const { settings, DEFAULT_SETTINGS } = await import('./stores/settings.svelte');

beforeEach(() => {
  calls.length = 0;
  Object.assign(settings, DEFAULT_SETTINGS);
});

it('turns shuffle on, then plays the list from a random track in its own order', async () => {
  const random = vi.spyOn(Math, 'random').mockReturnValue(0.6);
  await playShuffled(['a', 'b', 'c', 'd', 'e']);
  random.mockRestore();
  expect(calls).toEqual([
    ['settings_set', { patch: { shuffle: true } }],
    ['player_play', { trackIds: ['a', 'b', 'c', 'd', 'e'], startIndex: 3 }],
  ]);
  expect(settings.shuffle).toBe(true);
});

it('shuffles an album the same way', async () => {
  vi.spyOn(Math, 'random').mockReturnValue(0);
  await playAlbum('al1', { shuffled: true });
  vi.restoreAllMocks();
  expect(calls.map(([cmd]) => cmd)).toEqual(['library_album_tracks', 'settings_set', 'player_play']);
  expect(calls[2][1]).toEqual({ trackIds: ['t1', 't2', 't3', 't4'], startIndex: 0 });
});

it('plays nothing for an empty list', async () => {
  await playShuffled([]);
  expect(calls).toEqual([]);
});
