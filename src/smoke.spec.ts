import { flushSync, mount, unmount } from 'svelte';
import { expect, it, vi } from 'vitest';

const invoke = vi.fn(async (cmd: string) => {
  if (cmd === 'player_state') {
    return { status: 'idle', track: null, positionMs: 0, durationMs: 0, volume: 0.8, queueIndex: 0, queueLength: 0 };
  }
  if (cmd === 'window_vibrancy') return false;
  return undefined;
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: (cmd: string) => invoke(cmd) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isMaximized: async () => false,
    onResized: async () => () => {},
  }),
}));

const { default: App } = await import('./App.svelte');
const { setLang } = await import('./lib/i18n');

it('mounts the app shell with the idle Flow ring', () => {
  setLang('en');
  const target = document.createElement('div');
  const app = mount(App, { target });
  flushSync();
  expect(target.querySelector('.wordmark')?.textContent).toBe('Prism');
  expect(target.querySelector('h1')?.textContent).toBe('Home');
  expect(target.querySelector('.ring')?.getAttribute('aria-label')).toBe('Play all albums shuffled');
  expect(target.querySelector('.ring .label')?.textContent).toBe('Flow');
  unmount(app);
});

it('switches the strings with the language', () => {
  setLang('de');
  const target = document.createElement('div');
  const app = mount(App, { target });
  flushSync();
  expect(target.querySelector('h1')?.textContent).toBe('Start');
  setLang('en');
  flushSync();
  expect(target.querySelector('h1')?.textContent).toBe('Home');
  unmount(app);
});
