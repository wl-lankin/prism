import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
  if (cmd === 'settings_set') return { ...settings, ...(args as { patch: object }).patch };
  return undefined;
});
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

const { default: PlayModes } = await import('./PlayModes.svelte');
const { player } = await import('../stores/player.svelte');
const { settings, DEFAULT_SETTINGS } = await import('../stores/settings.svelte');
const { setLang } = await import('../i18n');

let component: ReturnType<typeof mount> | undefined;
let target: HTMLElement;

beforeEach(() => {
  setLang('en');
  invoke.mockClear();
  Object.assign(settings, DEFAULT_SETTINGS);
  Object.assign(player, { repeat: 'off', shuffle: false, flow: null, status: 'playing' });
  target = document.createElement('div');
  document.body.append(target);
});

afterEach(() => {
  if (component) unmount(component);
  component = undefined;
  target.remove();
});

function buttons() {
  const [shuffle, repeat] = [...target.querySelectorAll('button')];
  return { shuffle, repeat };
}

it('shows each mode with its own name and icon, and cycles repeat on click', async () => {
  component = mount(PlayModes, { target });
  flushSync();
  const { shuffle, repeat } = buttons();
  expect(shuffle.getAttribute('aria-label')).toBe('Shuffle: off');
  expect(shuffle.getAttribute('aria-pressed')).toBe('false');
  expect(repeat.getAttribute('aria-label')).toBe('Repeat: off');
  expect(repeat.title).toBe('Repeat: off');
  const offIcon = repeat.innerHTML;
  repeat.click();
  flushSync();
  expect(repeat.getAttribute('aria-label')).toBe('Repeat: all');
  expect(repeat.getAttribute('aria-pressed')).toBe('true');
  repeat.click();
  flushSync();
  expect(repeat.getAttribute('aria-label')).toBe('Repeat: this song');
  expect(repeat.innerHTML).not.toBe(offIcon);
  repeat.click();
  flushSync();
  expect(player.repeat).toBe('off');
  expect(repeat.getAttribute('aria-pressed')).toBe('false');
  await Promise.resolve();
  shuffle.click();
  flushSync();
  expect(shuffle.getAttribute('aria-pressed')).toBe('true');
  expect(shuffle.getAttribute('aria-label')).toBe('Shuffle: on');
});

it('is unavailable in Flow and says why', () => {
  // The modes are kept for later but do not apply: not pressed.
  Object.assign(player, { flow: { kind: 'favorites' }, repeat: 'all', shuffle: true });
  component = mount(PlayModes, { target });
  flushSync();
  const { shuffle, repeat } = buttons();
  expect(repeat.getAttribute('aria-disabled')).toBe('true');
  expect(repeat.title).toMatch(/Flow never ends/);
  expect(shuffle.getAttribute('aria-disabled')).toBe('true');
  expect(repeat.getAttribute('aria-pressed')).toBe('false');
  expect(shuffle.getAttribute('aria-pressed')).toBe('false');
  repeat.click();
  shuffle.click();
  flushSync();
  expect(invoke).not.toHaveBeenCalled();
  setLang('de');
  flushSync();
  expect(repeat.title).toMatch(/Flow endet nie/);
});
