// Repeat and shuffle: both are settings (saved, and sent to the engine by `settings_set`),
// shown from the player state. A Flow ignores both.

import { t } from './i18n';
import { player } from './stores/player.svelte';
import { updateSettings } from './stores/settings.svelte';
import type { RepeatMode } from './types';

const ORDER: RepeatMode[] = ['off', 'all', 'one'];

/** The mode after `mode`: off, all, one, off. */
export function nextRepeat(mode: RepeatMode): RepeatMode {
  return ORDER[(ORDER.indexOf(mode) + 1) % ORDER.length];
}

function inFlow(): boolean {
  return !!player.flow;
}

/** Shows the change at once; the engine's next state confirms it. */
async function save(patch: { repeat: RepeatMode } | { shuffle: boolean }, undo: () => void): Promise<void> {
  try {
    await updateSettings(patch);
  } catch (err) {
    undo();
    console.error('saving the play mode failed', err);
  }
}

/** Off, all, one, off. Nothing in Flow. */
export function cycleRepeat(): Promise<void> {
  if (inFlow()) return Promise.resolve();
  const before = player.repeat ?? 'off';
  const repeat = nextRepeat(before);
  player.repeat = repeat;
  return save({ repeat }, () => (player.repeat = before));
}

/** Shuffles the upcoming tracks, or restores their order. Nothing in Flow. */
export function toggleShuffle(): Promise<void> {
  if (inFlow()) return Promise.resolve();
  const shuffle = !player.shuffle;
  player.shuffle = shuffle;
  return save({ shuffle }, () => (player.shuffle = !shuffle));
}

/** The repeat button's name and tooltip. */
export function repeatLabel(mode: RepeatMode, flow: boolean): string {
  if (flow) return t('transport.repeatFlow');
  return t(`transport.repeat.${mode}`);
}

/** The shuffle button's name and tooltip. */
export function shuffleLabel(on: boolean, flow: boolean): string {
  if (flow) return t('transport.shuffleFlow');
  return t(on ? 'transport.shuffle.on' : 'transport.shuffle.off');
}
