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
 *  views that show it asks nothing again. */
export const lyricsState = $state<{ key: string | null; view: LyricsView }>({ key: null, view: { kind: 'idle' } });

type Target = { kind: 'idle' } | { kind: 'radio' } | { kind: 'load'; key: string; ask: () => Promise<LyricsAnswer> };

/** What the window plays now and how to ask for its lyrics. The online setting is part of
 *  the key: a "none" found with it off is worth asking again once it is on. */
function target(): Target {
  const online = settings.lyricsLookup ? 'online' : 'offline';
  if (player.active === 'ytm') {
    const { state } = ytm;
    if (ytmIdle(state)) return { kind: 'idle' };
    return {
      kind: 'load',
      key: `ytm:${state.title}|${state.artist}|${state.durationMs}|${online}`,
      ask: lyricsForYoutube,
    };
  }
  const track = player.track;
  if (player.status === 'idle' || !track) return { kind: 'idle' };
  if (isLive(track)) return { kind: 'radio' };
  return { kind: 'load', key: `track:${track.id}|${online}`, ask: () => lyricsForTrack(track.id) };
}

function start(key: string, ask: () => Promise<LyricsAnswer>): void {
  lyricsState.key = key;
  lyricsState.view = { kind: 'loading' };
  ask().then(
    (answer) => {
      if (lyricsState.key !== key) return;
      lyricsState.view =
        answer.status === 'found' ? { kind: 'found', lyrics: answer.lyrics } : { kind: answer.status };
    },
    (err) => {
      console.error('loading the lyrics failed', err);
      if (lyricsState.key === key) lyricsState.view = { kind: 'error' };
    },
  );
}

/** Brings `lyricsState` in line with what plays. Call it from an effect of a view that
 *  shows lyrics: it reads the player (so the effect re-runs on a track change) and asks
 *  only when the key is new. */
export function syncLyrics(): void {
  const next = target();
  untrack(() => {
    if (next.kind !== 'load') {
      lyricsState.key = null;
      lyricsState.view = { kind: next.kind };
    } else if (lyricsState.key !== next.key) {
      start(next.key, next.ask);
    }
  });
}

/** "Try again": asks again for the track that plays. */
export function retryLyrics(): void {
  const next = untrack(target);
  if (next.kind === 'load') start(next.key, next.ask);
}
