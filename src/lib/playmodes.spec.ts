import { beforeEach, expect, it, vi } from 'vitest';

const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
  if (cmd === 'settings_set') return { ...settings, ...(args as { patch: object }).patch };
  return undefined;
});
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

const { cycleRepeat, nextRepeat, toggleShuffle, repeatLabel, shuffleLabel } = await import('./playmodes');
const { player } = await import('./stores/player.svelte');
const { settings, DEFAULT_SETTINGS } = await import('./stores/settings.svelte');
const { setLang } = await import('./i18n');

beforeEach(() => {
  setLang('en');
  invoke.mockClear();
  Object.assign(settings, DEFAULT_SETTINGS);
  Object.assign(player, { repeat: 'off', shuffle: false, flow: null });
});

it('cycles off, all, one', () => {
  expect(nextRepeat('off')).toBe('all');
  expect(nextRepeat('all')).toBe('one');
  expect(nextRepeat('one')).toBe('off');
});

it('saves the next repeat mode and shows it at once', async () => {
  const saved = cycleRepeat();
  expect(player.repeat).toBe('all');
  await saved;
  expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { repeat: 'all' } });
  await cycleRepeat();
  expect(player.repeat).toBe('one');
});

it('toggles shuffle through the settings', async () => {
  await toggleShuffle();
  expect(player.shuffle).toBe(true);
  expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { shuffle: true } });
  await toggleShuffle();
  expect(player.shuffle).toBe(false);
});

it('does nothing in Flow', async () => {
  player.flow = { kind: 'library' };
  await cycleRepeat();
  await toggleShuffle();
  expect(invoke).not.toHaveBeenCalled();
  expect(player.repeat).toBe('off');
  expect(player.shuffle).toBe(false);
});

it('names each mode and explains Flow', () => {
  expect(repeatLabel('off', false)).toBe('Repeat: off');
  expect(repeatLabel('all', false)).toBe('Repeat: all');
  expect(repeatLabel('one', false)).toBe('Repeat: this song');
  expect(repeatLabel('all', true)).toMatch(/Flow never ends/);
  expect(shuffleLabel(true, false)).toBe('Shuffle: on');
  expect(shuffleLabel(false, false)).toBe('Shuffle: off');
  expect(shuffleLabel(false, true)).toMatch(/Flow/);
  setLang('de');
  expect(repeatLabel('one', false)).toBe('Wiederholen: diesen Song');
});
