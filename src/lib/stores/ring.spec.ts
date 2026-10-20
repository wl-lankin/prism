import { flushSync } from 'svelte';
import { expect, it, vi } from 'vitest';
import type { Track } from '../types';

type Handler = (event: { event: string; id: number; payload: unknown }) => void;

const listeners = new Map<string, Handler>();

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (event: string, handler: Handler) => {
    listeners.set(event, handler);
    return () => listeners.delete(event);
  }),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const GREEN: [string, string, string, string] = ['#3df57a', '#3df5c4', '#3dc4f5', '#7af53d'];
const ringColorsFor = vi.fn(async (coverId: string | null) => (coverId === 'green' ? GREEN : SPECTRUM));
vi.mock('../palette', async (original) => ({
  ...(await original<typeof import('../palette')>()),
  ringColorsFor: (coverId: string | null) => ringColorsFor(coverId),
}));

const { SPECTRUM } = await import('../palette');
const { player } = await import('./player.svelte');
const { ring, initRing } = await import('./ring.svelte');

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function level(value: number) {
  listeners.get('player://level')!({ event: 'player://level', id: 1, payload: value });
}

it('takes the colors of the playing cover and the level while playing', async () => {
  const stop = initRing();
  await tick();
  expect(ring.colors).toEqual(SPECTRUM);

  player.track = { id: 't', coverId: 'green' } as Track;
  player.status = 'playing';
  flushSync();
  await tick();
  expect(ring.colors).toEqual(GREEN);

  level(0.7);
  expect(ring.level).toBe(0.7);

  player.status = 'paused';
  flushSync();
  expect(ring.level).toBe(0);
  level(0.9);
  expect(ring.level, 'no level while paused').toBe(0);

  player.track = null;
  flushSync();
  await tick();
  expect(ring.colors).toEqual(SPECTRUM);

  stop();
  expect(listeners.has('player://level')).toBe(false);
});
