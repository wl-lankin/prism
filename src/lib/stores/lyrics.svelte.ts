import { untrack } from 'svelte';
import { isLive, ytmIdle } from '../format';
import { lyricsForTrack, lyricsForYoutube } from '../ipc';
import type { Lyrics, LyricsAnswer } from '../types';
import { player } from './player.svelte';
import { settings } from './settings.svelte';
import { ytm } from './ytm.svelte';

export type LyricsView =
  | { kind: 'idle' }
  | { kind: 'radio' }
  | { kind: 'loading' }
  | { kind: 'found'; lyrics: Lyrics }
  | { kind: 'none' }
  | { kind: 'error' };

/** The answer for the track that plays, under the key it was asked for. Only one is kept:
 *  an answer for a track that is no longer playing is dropped, and switching between the
 *  views that show it asks nothing again. `online` is the setting the answer was asked
 *  with. */
export const lyricsState = $state<{ key: string | null; online: boolean; view: LyricsView }>({
  key: null,
  online: true,
  view: { kind: 'idle' },
});

/** YouTube Music reports a song change in pieces (a new title with the old artist, a length
 *  of 0 until the media loads); a key must stay this long before it is asked for. */
export const SETTLE_MS = 300;

type Target =
  | { kind: 'idle' }
  | { kind: 'radio' }
  | { kind: 'wait' }
  | { kind: 'load'; key: string; settle: boolean; ask: () => Promise<LyricsAnswer> };

/** What the window plays now and how to ask for its lyrics. */
function target(): Target {
  if (player.active === 'ytm') {
    const { state } = ytm;
    if (ytmIdle(state)) return { kind: 'idle' };
    if (!state.title || !state.artist || state.durationMs <= 0) return { kind: 'wait' };
    const seconds = Math.round(state.durationMs / 1000);
    return { kind: 'load', key: `ytm:${state.title}|${state.artist}|${seconds}`, settle: true, ask: lyricsForYoutube };
  }
  const track = player.track;
  if (player.status === 'idle' || !track) return { kind: 'idle' };
  if (isLive(track)) return { kind: 'radio' };
  return { kind: 'load', key: `track:${track.id}`, settle: false, ask: () => lyricsForTrack(track.id) };
}

/** Counts requests; an answer counts only while it belongs to the latest one. */
let seq = 0;
let settleTimer: ReturnType<typeof setTimeout> | undefined;
let settling: string | null = null;

function cancelSettle(): void {
  clearTimeout(settleTimer);
  settleTimer = undefined;
  settling = null;
}

function start(key: string, ask: () => Promise<LyricsAnswer>): void {
  cancelSettle();
  const mine = ++seq;
  lyricsState.key = key;
  lyricsState.online = settings.lyricsLookup;
  lyricsState.view = { kind: 'loading' };
  ask().then(
    (answer) => {
      if (mine !== seq) return;
      lyricsState.view =
        answer.status === 'found' ? { kind: 'found', lyrics: answer.lyrics } : { kind: answer.status };
    },
    (err) => {
      console.error('loading the lyrics failed', err);
      if (mine === seq) lyricsState.view = { kind: 'error' };
    },
  );
}

function show(view: LyricsView): void {
  cancelSettle();
  seq++;
  lyricsState.key = null;
  lyricsState.view = view;
}

/** Brings `lyricsState` in line with what plays. Call it from an effect of a view that
 *  shows lyrics: it reads the player (so the effect re-runs on a track change) and asks
 *  only when the key is new. Switching the online search only asks again for an answer
 *  that was none or an error. */
export function syncLyrics(): void {
  const next = target();
  // Read outside untrack: the effect re-runs when the setting changes.
  const online = settings.lyricsLookup;
  untrack(() => {
    if (next.kind === 'wait') return show({ kind: 'loading' });
    if (next.kind !== 'load') return show({ kind: next.kind });
    if (lyricsState.key === next.key) {
      cancelSettle();
      if (online === lyricsState.online) return;
      const { kind } = lyricsState.view;
      if (kind === 'none' || kind === 'error') start(next.key, next.ask);
      else lyricsState.online = online;
      return;
    }
    if (!next.settle) return start(next.key, next.ask);
    if (settling === next.key) return;
    cancelSettle();
    settling = next.key;
    seq++;
    lyricsState.key = null;
    lyricsState.view = { kind: 'loading' };
    settleTimer = setTimeout(() => start(next.key, next.ask), SETTLE_MS);
  });
}

/** "Try again": asks again for the track that plays. */
export function retryLyrics(): void {
  const next = untrack(target);
  if (next.kind === 'load') start(next.key, next.ask);
}
