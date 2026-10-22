import { t } from './i18n';
import { flowStart } from './ipc';
import { go } from './stores/nav.svelte';
import { notify } from './stores/notice.svelte';
import type { FlowVariant } from './types';

/** How long a press on the ring takes to open the Flow menu. */
export const HOLD_MS = 500;
/** A press that moves further than this (px) is a drag, not a hold. */
const HOLD_SLOP = 8;

/** Fisher-Yates, in place. */
export function shuffle<T>(items: T[]): T[] {
  for (let i = items.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [items[i], items[j]] = [items[j], items[i]];
  }
  return items;
}

/**
 * Starts an endless Flow of `variant`. Favorites without any fall back to the whole library
 * (with a notice); an empty library goes to Settings; a failure shows the backend's message.
 */
export async function playFlow(variant: FlowVariant): Promise<'started' | 'empty' | 'failed'> {
  try {
    const outcome = await flowStart(variant);
    if (outcome.outcome === 'empty') {
      go('settings');
      return 'empty';
    }
    if (variant.kind === 'favorites' && outcome.variant.kind === 'library') notify(t('flow.noFavorites'));
    return 'started';
  } catch (err) {
    notify(t('flow.failed', { message: String(err) }));
    return 'failed';
  }
}

/** What a click on the idle ring does. */
export const activateIdle = () => playFlow({ kind: 'favorites' });

/** Two Flow variants are the same mix (genres compare case-insensitively, as the pool does). */
export function sameFlow(a: FlowVariant | null | undefined, b: FlowVariant): boolean {
  if (!a || a.kind !== b.kind) return false;
  return a.kind !== 'genre' || b.kind !== 'genre' || a.genre.toLowerCase() === b.genre.toLowerCase();
}

/**
 * Calls `onhold` with the press point after a primary-button press of `HOLD_MS` on `node`
 * (the long-press way to the Flow menu) and swallows the click that ends that press, so the
 * ring is not also activated. A short press stays a plain click.
 */
export function holdToOpen(node: HTMLElement, onhold: (x: number, y: number) => void) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let held = false;
  let start: [number, number] = [0, 0];

  const cancel = () => {
    clearTimeout(timer);
    timer = undefined;
  };
  const down = (event: PointerEvent) => {
    if (event.button !== 0 || !event.isPrimary) return;
    cancel();
    held = false;
    start = [event.clientX, event.clientY];
    timer = setTimeout(() => {
      timer = undefined;
      held = true;
      onhold(start[0], start[1]);
    }, HOLD_MS);
  };
  const move = (event: PointerEvent) => {
    if (timer && Math.hypot(event.clientX - start[0], event.clientY - start[1]) > HOLD_SLOP) cancel();
  };
  const click = (event: MouseEvent) => {
    if (!held) return;
    held = false;
    event.preventDefault();
    event.stopPropagation();
  };

  node.addEventListener('pointerdown', down);
  node.addEventListener('pointermove', move);
  node.addEventListener('pointerup', cancel);
  node.addEventListener('pointerleave', cancel);
  node.addEventListener('pointercancel', cancel);
  node.addEventListener('click', click, true);
  return {
    update(next: (x: number, y: number) => void) {
      onhold = next;
    },
    destroy() {
      cancel();
      node.removeEventListener('pointerdown', down);
      node.removeEventListener('pointermove', move);
      node.removeEventListener('pointerup', cancel);
      node.removeEventListener('pointerleave', cancel);
      node.removeEventListener('pointercancel', cancel);
      node.removeEventListener('click', click, true);
    },
  };
}
