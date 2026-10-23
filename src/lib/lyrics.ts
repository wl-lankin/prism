// Lyrics arithmetic: which line is active and where the drawn position is.

import type { LyricsLine } from './types';

/** A line lights up this long before its time, so the eye is on it when it is sung. */
export const LEAD_MS = 150;

/** How long a manual scroll keeps the view from following the song. */
export const FOLLOW_PAUSE_MS = 5000;

/** The last line with `startMs <= positionMs + LEAD_MS`, or -1 before the first one. The
 *  lines are in time order. */
export function activeLine(lines: readonly LyricsLine[], positionMs: number): number {
  const at = positionMs + LEAD_MS;
  let low = 0;
  let high = lines.length - 1;
  let found = -1;
  while (low <= high) {
    const mid = (low + high) >> 1;
    if ((lines[mid].startMs ?? Infinity) <= at) {
      found = mid;
      low = mid + 1;
    } else {
      high = mid - 1;
    }
  }
  return found;
}

/** A position reported at `base.at` (a `performance.now()` value) moves on with the clock
 *  while the song plays, and stays put while it does not. */
export function interpolate(
  base: { positionMs: number; at: number },
  now: number,
  playing: boolean,
  durationMs: number,
): number {
  const ms = playing ? base.positionMs + Math.max(0, now - base.at) : base.positionMs;
  return durationMs > 0 ? Math.min(ms, durationMs) : ms;
}
