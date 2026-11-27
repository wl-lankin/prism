import { beforeEach, expect, it, vi } from 'vitest';

let maximized = false;
let fullscreen = false;
let resized: (() => void) | undefined;
const unlisten = vi.fn();
const isMaximized = vi.fn(async () => maximized);
const isFullscreen = vi.fn(async () => fullscreen);

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isMaximized,
    isFullscreen,
    onResized: async (handler: () => void) => {
      resized = handler;
      return unlisten;
    },
  }),
}));

const { windowState, initWindow } = await import('./window.svelte');

beforeEach(() => {
  maximized = false;
  fullscreen = false;
  resized = undefined;
  unlisten.mockReset();
  isMaximized.mockClear();
  isFullscreen.mockClear();
});

it('starts restored', () => {
  expect(windowState.maximized).toBe(false);
});

it('follows onResized and isMaximized', async () => {
  maximized = true;
  const stop = await initWindow();
  expect(windowState.maximized).toBe(true);

  maximized = false;
  resized!();
  await vi.waitFor(() => expect(windowState.maximized).toBe(false));

  maximized = true;
  resized!();
  await vi.waitFor(() => expect(windowState.maximized).toBe(true));
  expect(isMaximized).toHaveBeenCalledTimes(3);

  stop();
  expect(unlisten).toHaveBeenCalled();
});

it('keeps the last value when asking fails', async () => {
  maximized = true;
  const stop = await initWindow();
  expect(windowState.maximized).toBe(true);
  isMaximized.mockRejectedValueOnce(new Error('gone'));
  resized!();
  await Promise.resolve();
  expect(windowState.maximized).toBe(true);
  stop();
});

it('counts a full screen window as maximized (the kiosk)', async () => {
  fullscreen = true;
  const stop = await initWindow();
  expect(windowState.maximized).toBe(true);

  fullscreen = false;
  resized!();
  await vi.waitFor(() => expect(windowState.maximized).toBe(false));
  stop();
});

it('starts outside kiosk mode', () => {
  expect(windowState.kiosk).toBe(false);
});
