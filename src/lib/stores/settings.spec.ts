import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { Settings } from '../types';

type Handler = (event: { payload: unknown }) => void;
const handlers = new Map<string, Handler>();
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, handler: Handler) => {
    handlers.set(name, handler);
    return () => handlers.delete(name);
  }),
}));

let answer: (value: unknown) => void = () => {};
const invoke = vi.fn(async (cmd: string, _args?: unknown): Promise<unknown> => {
  if (cmd === 'settings_set') return new Promise((resolve) => (answer = resolve));
  return undefined;
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: (cmd: string, args?: unknown) => invoke(cmd, args) }));

const { settings, followSettings, updateSettings, DEFAULT_SETTINGS } = await import('./settings.svelte');
const { lang, setLang } = await import('../i18n');

let stop: () => void;
beforeEach(async () => {
  setLang('en');
  Object.assign(settings, DEFAULT_SETTINGS, { language: 'en' });
  stop = await followSettings();
});
afterEach(() => stop());

const changed = (patch: Partial<Settings>) =>
  handlers.get('settings://changed')!({ payload: { ...DEFAULT_SETTINGS, language: 'en', ...patch } });

it('takes settings changed in another window, the language included', () => {
  changed({ language: 'de', miniOnTop: true });
  expect(settings.language).toBe('de');
  expect(settings.miniOnTop).toBe(true);
  expect(lang()).toBe('de');
});

it('keeps a change of its own that is still on its way', async () => {
  const saving = updateSettings({ volume: 0.3 });
  // Another window's save arrives first, with the old volume.
  changed({ volume: 0.8, gapless: false });
  expect(settings.volume).toBe(0.3);
  expect(settings.gapless).toBe(false);
  answer({ ...DEFAULT_SETTINGS, language: 'en', volume: 0.3, gapless: false });
  await saving;
  expect(settings.volume).toBe(0.3);
  // Settled: the next event applies again.
  changed({ volume: 0.5 });
  expect(settings.volume).toBe(0.5);
});
