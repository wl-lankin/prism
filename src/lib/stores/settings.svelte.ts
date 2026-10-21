import { setLang } from '../i18n';
import { onSettingsChanged, settingsGet, settingsSet } from '../ipc';
import type { Settings } from '../types';

export const DEFAULT_SETTINGS: Settings = {
  language: 'system',
  volume: 0.8,
  outputDevice: null,
  gapless: true,
  replayGain: 'off',
  crossfade: 'flow',
  crossfadeMs: 4000,
  coverLookup: true,
  miniOnTop: false,
  miniShortcut: 'CommandOrControl+Alt+P',
  startWithSystem: false,
  repeat: 'off',
  shuffle: false,
  navidromeUrl: null,
  navidromeUser: null,
  navidromeQuality: 'original',
  radioLocation: { mode: 'auto' },
};

/** The saved settings as the UI shows them. Changes go through `updateSettings`. */
export const settings: Settings = $state({ ...DEFAULT_SETTINGS });

/** Copies only `keys` from `source`, so an answer to one change never overwrites another
 *  change made meanwhile. */
function assign(keys: (keyof Settings)[], source: Settings): void {
  const target = settings as Record<keyof Settings, unknown>;
  for (const key of keys) target[key] = source[key];
  if (keys.includes('language')) setLang(settings.language);
}

/** Loads the saved settings and applies the language. The engine already started with the
 *  saved volume, device and playback options. */
export async function initSettings(): Promise<void> {
  const saved = await settingsGet();
  if (saved) assign(Object.keys(DEFAULT_SETTINGS) as (keyof Settings)[], saved);
}

/** Keys with a save of this window still on its way, and how many saves each. */
const saving = new Map<keyof Settings, number>();

/** Shows `patch` at once, saves it and takes what the backend stored for those keys. On a
 *  failure those keys get their previous values back and the promise rejects. */
export async function updateSettings(patch: Partial<Settings>): Promise<void> {
  const keys = Object.keys(patch) as (keyof Settings)[];
  const before = { ...settings };
  for (const key of keys) saving.set(key, (saving.get(key) ?? 0) + 1);
  assign(keys, { ...settings, ...patch });
  try {
    const saved = await settingsSet(patch);
    if (saved) assign(keys, saved);
  } catch (err) {
    assign(keys, before);
    throw err;
  } finally {
    for (const key of keys) {
      const left = (saving.get(key) ?? 1) - 1;
      if (left > 0) saving.set(key, left);
      else saving.delete(key);
    }
  }
}

/** Follows `settings://changed`, which the backend sends after every save (from either
 *  window), so a language picked in the main window reaches the mini player at once. Keys this
 *  window is still saving keep the value it shows; its own answer settles them. Resolves to
 *  a function that stops following. */
export async function followSettings(): Promise<() => void> {
  return onSettingsChanged((saved) => {
    if (!saved) return;
    const keys = (Object.keys(DEFAULT_SETTINGS) as (keyof Settings)[]).filter(
      (key) => !saving.has(key) && key in saved && settings[key] !== saved[key],
    );
    if (keys.length) assign(keys, saved);
  });
}
