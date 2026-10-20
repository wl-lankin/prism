import { getCurrentWindow } from '@tauri-apps/api/window';
import { setLang } from '../i18n';
import { settingsGet, settingsSet } from '../ipc';
import type { Settings } from '../types';

export const DEFAULT_SETTINGS: Settings = {
  language: 'system',
  volume: 0.8,
  outputDevice: null,
  gapless: true,
  replayGain: 'off',
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

let closeHooked = false;

/** Loads the saved settings and applies the language. The engine already started with the
 *  saved volume, device and playback options. Also makes sure a pending volume save is
 *  written before the window closes or reloads. */
export async function initSettings(): Promise<void> {
  hookClose();
  const saved = await settingsGet();
  if (saved) assign(Object.keys(DEFAULT_SETTINGS) as (keyof Settings)[], saved);
}

/** Shows `patch` at once, saves it and takes what the backend stored for those keys. On a
 *  failure those keys get their previous values back and the promise rejects. */
export async function updateSettings(patch: Partial<Settings>): Promise<void> {
  const keys = Object.keys(patch) as (keyof Settings)[];
  const before = { ...settings };
  assign(keys, { ...settings, ...patch });
  try {
    const saved = await settingsSet(patch);
    if (saved) assign(keys, saved);
  } catch (err) {
    assign(keys, before);
    throw err;
  }
}

/** How long the volume has to stay put before it is written to disk. */
export const VOLUME_SAVE_DELAY = 400;
let volumeTimer: ReturnType<typeof setTimeout> | undefined;
let pendingVolume: number | null = null;

/** Remembers the volume once it has settled (a slider drag sends many values). The player
 *  itself is set separately and at once. */
export function saveVolume(volume: number): void {
  pendingVolume = volume;
  clearTimeout(volumeTimer);
  volumeTimer = setTimeout(() => void flushVolume(), VOLUME_SAVE_DELAY);
}

/** Writes a volume that is still waiting for its debounce, now. */
export async function flushVolume(): Promise<void> {
  clearTimeout(volumeTimer);
  if (pendingVolume === null) return;
  const volume = pendingVolume;
  pendingVolume = null;
  settings.volume = volume;
  try {
    await settingsSet({ volume });
  } catch (err) {
    console.error('saving the volume failed', err);
  }
}

function hookClose(): void {
  if (closeHooked || typeof window === 'undefined') return;
  closeHooked = true;
  window.addEventListener('pagehide', () => void flushVolume());
  try {
    // The window is destroyed once this resolves.
    getCurrentWindow()
      .onCloseRequested(async () => {
        await flushVolume();
      })
      .catch(() => {});
  } catch {
    // Not inside Tauri (tests).
  }
}
