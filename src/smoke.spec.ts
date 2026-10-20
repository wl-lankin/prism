import { flushSync, mount, unmount } from 'svelte';
import { expect, it, vi } from 'vitest';

const invoke = vi.fn(async (cmd: string) => {
  if (cmd === 'player_state') {
    return { status: 'idle', track: null, positionMs: 0, durationMs: 0, volume: 0.8, queueIndex: 0, queueLength: 0, queueVersion: 0 };
  }
  if (cmd === 'window_vibrancy') return false;
  return undefined;
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: (cmd: string) => invoke(cmd) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(async () => null) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isMaximized: async () => false,
    onResized: async () => () => {},
    onDragDropEvent: async () => () => {},
  }),
}));

const { default: App } = await import('./App.svelte');
const { setLang } = await import('./lib/i18n');

it('mounts the app shell with the idle Flow ring and the empty library', async () => {
  setLang('en');
  const target = document.createElement('div');
  const app = mount(App, { target });
  flushSync();
  expect(target.querySelector('.wordmark')?.textContent).toBe('Prism');
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelector('h1')?.textContent).toBe('Your library is empty');
  });
  expect(target.querySelector('.ring')?.getAttribute('aria-label')).toBe('Play all albums shuffled');
  expect(target.querySelector('.ring .label')?.textContent).toBe('Flow');
  unmount(app);
});

it('puts the ring first in the tab order and names disabled entries', () => {
  setLang('en');
  const target = document.createElement('div');
  const app = mount(App, { target });
  flushSync();
  expect(target.querySelector('button')?.classList.contains('ring')).toBe(true);
  const items = [...target.querySelectorAll('.item')];
  const navidrome = items.find((b) => b.textContent?.includes('Navidrome'));
  expect(navidrome?.getAttribute('aria-disabled')).toBe('true');
  expect(navidrome?.textContent).toContain('Coming soon');
  const playlists = items.find((b) => b.textContent?.includes('Playlists'));
  expect(playlists?.getAttribute('aria-disabled')).toBeNull();
  expect(playlists?.textContent).not.toContain('Coming soon');
  const favorites = items.find((b) => b.textContent?.includes('Favorites'));
  expect(favorites?.getAttribute('aria-disabled')).toBeNull();
  expect(favorites?.textContent).not.toContain('Coming soon');
  expect(target.querySelector('[role="status"]')).not.toBeNull();
  expect(document.documentElement.lang).toBe('en');
  unmount(app);
});

it('shows the coming-soon pill outside the clipping sidebar list on hover', () => {
  setLang('de');
  const target = document.createElement('div');
  const app = mount(App, { target });
  flushSync();
  const navidrome = [...target.querySelectorAll<HTMLElement>('.item')].find((b) => b.textContent?.includes('Navidrome'));
  expect(navidrome?.textContent).toContain('Kommt bald');
  const pill = target.querySelector('.soon-tip');
  expect(pill).not.toBeNull();
  expect(pill?.closest('.scroll')).toBeNull();
  expect(pill?.getAttribute('aria-hidden')).toBe('true');
  expect(pill?.classList.contains('shown')).toBe(false);
  navidrome?.dispatchEvent(new PointerEvent('pointerenter'));
  flushSync();
  expect(pill?.classList.contains('shown')).toBe(true);
  expect(pill?.textContent).toBe('Kommt bald');
  navidrome?.dispatchEvent(new PointerEvent('pointerleave'));
  flushSync();
  expect(pill?.classList.contains('shown')).toBe(false);
  unmount(app);
  setLang('en');
});

it('switches the strings with the language', async () => {
  setLang('de');
  const target = document.createElement('div');
  const app = mount(App, { target });
  flushSync();
  expect(target.querySelector('h1')?.textContent).toBe('Deine Mediathek ist leer');
  setLang('en');
  flushSync();
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelector('h1')?.textContent).toBe('Your library is empty');
  });
  unmount(app);
});
