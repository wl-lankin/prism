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

function apply(next: Settings): void {
  Object.assign(settings, next);
  setLang(next.language);
}

/** Loads the saved settings and applies the language. The engine already started with the
 *  saved volume, device and playback options. */
export async function initSettings(): Promise<void> {
  const saved = await settingsGet();
  if (saved) apply(saved);
}

/** Shows `patch` at once, saves it and takes what the backend stored. On a failure the
 *  previous values come back and the promise rejects. */
export async function updateSettings(patch: Partial<Settings>): Promise<void> {
  const before = { ...settings };
  Object.assign(settings, patch);
  if (patch.language) setLang(patch.language);
  try {
    const saved = await settingsSet(patch);
    if (saved) apply(saved);
  } catch (err) {
    apply(before);
    throw err;
  }
}

/** How long the volume has to stay put before it is written to disk. */
export const VOLUME_SAVE_DELAY = 400;
let volumeTimer: ReturnType<typeof setTimeout> | undefined;

/** Remembers the volume once it has settled (a slider drag sends many values). The player
 *  itself is set separately and at once. */
export function saveVolume(volume: number): void {
  clearTimeout(volumeTimer);
  volumeTimer = setTimeout(() => {
    settings.volume = volume;
    settingsSet({ volume }).catch((err) => console.error('saving the volume failed', err));
  }, VOLUME_SAVE_DELAY);
}
