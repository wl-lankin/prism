import { onYtmQueue, onYtmState, ytmHide, ytmQueue, ytmQueuePlay, ytmShow, ytmSignIn, ytmSignOut, ytmState } from '../ipc';
import type { YtmQueue, YtmRect, YtmState } from '../types';

const IDLE: YtmState = {
  status: 'idle',
  title: '',
  artist: '',
  album: '',
  coverId: null,
  positionMs: 0,
  durationMs: 0,
  canNext: false,
  canPrev: false,
};

/** YouTube Music as the window knows it. `shown`: the view asked for the child view (it
 *  is mounted). `hiddenForOverlay`: the child is hidden only because an overlay is open;
 *  it comes back with the last rect when the last overlay closes. Call `initYtm()` once
 *  per window. */
export const ytm = $state({
  state: { ...IDLE } as YtmState,
  /** YouTube Music's own queue, as its page shows it. */
  queue: { items: [], available: false } as YtmQueue,
  shown: false,
  hiddenForOverlay: false,
  /** The position to draw: the page reports once a second, so while it plays this runs on
   *  between reports instead of jumping (owner, 2026-10-22). */
  positionMs: 0,
});

/** How often the drawn position moves on while the page plays. */
const TICK_MS = 100;
let base = { positionMs: 0, at: 0 };
let ticker: ReturnType<typeof setInterval> | undefined;

function tick(): void {
  const { durationMs } = ytm.state;
  const now = base.positionMs + (performance.now() - base.at);
  ytm.positionMs = durationMs > 0 ? Math.min(now, durationMs) : now;
}

/** Takes a state from the page and keeps the drawn position running while it plays. */
export function applyState(state: YtmState): void {
  ytm.state = state;
  moveTo(state.positionMs);
}

/** Puts the drawn position at `ms` now (a report, or a seek by the user). */
export function moveTo(ms: number): void {
  base = { positionMs: ms, at: performance.now() };
  ytm.positionMs = ms;
  if (ytm.state.status === 'playing') {
    ticker ??= setInterval(tick, TICK_MS);
  } else if (ticker !== undefined) {
    clearInterval(ticker);
    ticker = undefined;
  }
}

/** The latest rect the view asked for, and the one the backend has. */
let wanted: YtmRect | null = null;
let sent: YtmRect | null = null;
let framePending = false;
let overlayOpen = false;

const same = (a: YtmRect, b: YtmRect) =>
  a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;

function send(rect: YtmRect): void {
  if (sent && same(sent, rect)) return;
  sent = { ...rect };
  ytmShow(rect).catch((err) => {
    sent = null;
    console.error('showing the YouTube Music view failed', err);
  });
}

function flush(): void {
  framePending = false;
  if (!ytm.shown || overlayOpen || !wanted) return;
  send(wanted);
}

/** The placeholder's rect in logical pixels. At most one `ytm_show` goes out per animation
 *  frame and the latest rect wins; a rect the backend already has is not sent again. */
export function reportRect(rect: YtmRect): void {
  wanted = { ...rect };
  ytm.shown = true;
  if (overlayOpen) {
    ytm.hiddenForOverlay = true;
    return;
  }
  if (framePending) return;
  framePending = true;
  requestAnimationFrame(flush);
}

/** The view is left: the child is hidden (not destroyed) and the next rect shows it again. */
export function hideView(): void {
  if (!ytm.shown) return;
  const visible = sent !== null;
  ytm.shown = false;
  ytm.hiddenForOverlay = false;
  wanted = null;
  sent = null;
  if (visible) ytmHide().catch((err) => console.error('hiding the YouTube Music view failed', err));
}

/** Called by the overlay registry when the first overlay opens and the last one closes. */
export function setOverlayOpen(open: boolean): void {
  overlayOpen = open;
  if (open) {
    if (!ytm.shown || ytm.hiddenForOverlay) return;
    ytm.hiddenForOverlay = true;
    // Nothing to hide before the first show went out; the pending rect waits for the close.
    if (sent === null) return;
    sent = null;
    ytmHide().catch((err) => console.error('hiding the YouTube Music view failed', err));
  } else if (ytm.hiddenForOverlay) {
    ytm.hiddenForOverlay = false;
    if (wanted) send(wanted);
  }
}

export async function signIn(): Promise<void> {
  try {
    await ytmSignIn();
  } catch (err) {
    console.error('opening the YouTube Music sign-in failed', err);
  }
}

/** Destroys the view and forgets the session; the caller asks first. */
export async function signOut(): Promise<void> {
  try {
    await ytmSignOut();
  } catch (err) {
    console.error('signing out of YouTube Music failed', err);
  }
}

/** Plays entry `index` of YouTube Music's queue in its page. */
export async function playQueueEntry(index: number): Promise<void> {
  try {
    await ytmQueuePlay(index);
  } catch (err) {
    console.error('playing from the YouTube Music queue failed', err);
  }
}

let stopCurrent: (() => void) | null = null;

/** Follows `ytm://state` and `ytm://queue` and loads both. An event
 *  that arrives while the initial answer is in flight is newer and wins. Resolves with the
 *  cleanup; a second call replaces the first one's listeners. */
export async function initYtm(): Promise<() => void> {
  stopCurrent?.();
  const stops: Array<() => void> = [];
  let stopped = false;
  const stop = () => {
    if (stopped) return;
    stopped = true;
    stops.splice(0).forEach((fn) => fn());
    if (stopCurrent === stop) stopCurrent = null;
  };
  stopCurrent = stop;
  const keep = (unlisten: () => void) => (stopped ? unlisten() : stops.push(unlisten));

  let heard = false;
  try {
    keep(
      await onYtmState((state) => {
        heard = true;
        applyState(state);
      }),
    );
  } catch (err) {
    console.error('following YouTube Music failed', err);
  }
  try {
    const state = await ytmState();
    if (!heard && !stopped && state) applyState(state);
  } catch (err) {
    console.error('loading the YouTube Music state failed', err);
  }

  let heardQueue = false;
  try {
    keep(
      await onYtmQueue((queue) => {
        heardQueue = true;
        ytm.queue = queue;
      }),
    );
  } catch (err) {
    console.error('following the YouTube Music queue failed', err);
  }
  try {
    const queue = await ytmQueue();
    if (!heardQueue && !stopped && queue) ytm.queue = queue;
  } catch (err) {
    console.error('loading the YouTube Music queue failed', err);
  }
  return stop;
}
