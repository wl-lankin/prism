import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { PlayerState } from '../types';

type Handler = (event: { event: string; id: number; payload: unknown }) => void;

const listeners = new Map<string, Handler>();
const unlisten = vi.fn();

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (event: string, handler: Handler) => {
    listeners.set(event, handler);
    return unlisten;
  }),
}));

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const { player, initPlayer } = await import('./player.svelte');

const playing: PlayerState = {
  status: 'playing',
  track: null,
  positionMs: 1200,
  durationMs: 5000,
  volume: 0.5,
  queueIndex: 2,
  queueLength: 4,
};

beforeEach(() => {
  listeners.clear();
  invoke.mockReset();
  unlisten.mockReset();
});

afterEach(() => vi.clearAllMocks());

it('starts idle', () => {
  expect(player.status).toBe('idle');
  expect(player.track).toBeNull();
});

it('follows player://state events', async () => {
  invoke.mockResolvedValue({ ...playing, status: 'paused' });
  const stop = await initPlayer();
  expect(invoke).toHaveBeenCalledWith('player_state', undefined);
  expect(player.status).toBe('paused');

  listeners.get('player://state')!({ event: 'player://state', id: 1, payload: playing });
  expect(player.status).toBe('playing');
  expect(player.positionMs).toBe(1200);
  expect(player.queueIndex).toBe(2);

  stop();
  expect(unlisten).toHaveBeenCalled();
});

it('an event that arrives before the first player_state wins', async () => {
  let resolve!: (state: PlayerState) => void;
  invoke.mockReturnValue(new Promise<PlayerState>((r) => (resolve = r)));
  const started = initPlayer();
  await vi.waitFor(() => expect(listeners.has('player://state')).toBe(true));
  listeners.get('player://state')!({
    event: 'player://state',
    id: 1,
    payload: { ...playing, positionMs: 3000 },
  });
  resolve({ ...playing, status: 'loading', positionMs: 0 });
  const stop = await started;
  expect(player.status).toBe('playing');
  expect(player.positionMs).toBe(3000);
  stop();
});
