import { beforeEach, expect, it, vi } from 'vitest';

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

const { library, initLibrary } = await import('./library.svelte');

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
