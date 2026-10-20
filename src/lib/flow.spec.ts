import { beforeEach, expect, it, vi } from 'vitest';

const invoke = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const { FLOW_CAP, playAllShuffled, shuffle } = await import('./flow');

function library(albums: number, tracksPerAlbum: number) {
  invoke.mockImplementation(async (...call: unknown[]) => {
    const [cmd, args] = call as [string, { albumId?: string } | undefined];
    if (cmd === 'library_albums') return Array.from({ length: albums }, (_, i) => ({ id: `a${i}` }));
    if (cmd === 'library_album_tracks') {
      return Array.from({ length: tracksPerAlbum }, (_, i) => ({ id: `${args?.albumId}-t${i}` }));
    }
    if (cmd === 'player_play') return undefined;
    throw new Error(`unexpected ${JSON.stringify([cmd, args])}`);
  });
}

beforeEach(() => {
  invoke.mockReset();
});

it('reports an empty library without playing', async () => {
  library(0, 0);
  expect(await playAllShuffled()).toBe('empty');
  expect(invoke).not.toHaveBeenCalledWith('player_play', expect.anything());
});

it('plays every track once, from the first', async () => {
  library(3, 4);
  expect(await playAllShuffled()).toBe('played');
  const play = invoke.mock.calls.find(([cmd]) => cmd === 'player_play');
  const { trackIds, startIndex } = play![1] as { trackIds: string[]; startIndex: number };
  expect(startIndex).toBe(0);
  expect(new Set(trackIds).size).toBe(12);
});

it('caps the queue', async () => {
  library(30, 100);
  await playAllShuffled();
  const play = invoke.mock.calls.find(([cmd]) => cmd === 'player_play');
  expect((play![1] as { trackIds: string[] }).trackIds).toHaveLength(FLOW_CAP);
});

it('shuffles without losing or duplicating items', () => {
  const items = Array.from({ length: 50 }, (_, i) => i);
  const out = shuffle([...items]);
  expect([...out].sort((a, b) => a - b)).toEqual(items);
});
