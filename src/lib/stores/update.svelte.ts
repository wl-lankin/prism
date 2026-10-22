import { t } from '../i18n';
import { onUpdateState, updateCheck, updateRestart, updateState } from '../ipc';
import type { UpdateState } from '../types';
import { notify } from './notice.svelte';

/** The update service's state as the main window shows it. Replaced from `update_state` and
 *  every `update://state`. */
export const update: UpdateState = $state({
  status: 'idle',
  version: null,
  notes: null,
  progress: null,
  lastCheck: null,
  installsOnQuit: false,
});

/** The version the notice already announced, so a check every 6 hours does not repeat it. */
let announced: string | null = null;

function apply(next: UpdateState): void {
  Object.assign(update, next);
  if (next.status === 'available' && next.version && next.version !== announced) {
    announced = next.version;
    notify(t('update.availableNotice', { version: next.version }));
  }
}

/** Loads the state, then follows `update://state`. Resolves with the cleanup. */
export async function initUpdate(): Promise<() => void> {
  let heard = false;
  let stop: (() => void) | undefined;
  try {
    stop = await onUpdateState((state) => {
      if (!state) return;
      heard = true;
      apply(state);
    });
  } catch (err) {
    console.error('following the update state failed', err);
  }
  try {
    const state = await updateState();
    if (state && !heard) apply(state);
  } catch (err) {
    console.error('loading the update state failed', err);
  }
  return () => stop?.();
}

/** "Check now", and "Download" for an update that was only announced. */
export async function checkForUpdates(): Promise<void> {
  try {
    await updateCheck();
  } catch (err) {
    console.error('checking for updates failed', err);
  }
}

/** "Restart to update". On success Prism quits and starts again. */
export async function restartToUpdate(): Promise<void> {
  if (restarting) return;
  restarting = true;
  try {
    await updateRestart();
  } catch (err) {
    console.error('installing the update failed', err);
    notify(t('update.restartFailed'));
  } finally {
    restarting = false;
  }
}

/** A second click while the install runs would find it already done and report a failure. */
let restarting = false;

/** The status line Settings shows. */
export function updateStatusText(state: UpdateState): string | null {
  const version = state.version ?? '';
  switch (state.status) {
    case 'checking':
      return t('update.checking');
    case 'upToDate':
      return t('update.upToDate');
    case 'available':
      return t('update.available', { version });
    case 'downloading':
      return state.progress === null
        ? t('update.downloading', { version })
        : t('update.downloadingPercent', { version, percent: Math.round(state.progress * 100) });
    case 'ready':
      return t(state.installsOnQuit ? 'update.ready' : 'update.readyRestart', { version });
    case 'failed':
      return t('update.failed');
    default:
      return null;
  }
}
