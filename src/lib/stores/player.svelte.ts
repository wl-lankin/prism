import { onPlayerState, playerState } from '../ipc';
import type { PlayerState } from '../types';

/** The player as the engine last reported it. Call `initPlayer()` once per window. */
export const player: PlayerState = $state({
  status: 'idle',
  track: null,
  positionMs: 0,
  durationMs: 0,
  volume: 0.8,
  queueIndex: 0,
  queueLength: 0,
  queueVersion: 0,
});

/**
 * Copies a new state in. The track object is only replaced when the track changes, so
 * readers of `player.track` do not re-run on every 250 ms position update.
 */
function apply(state: PlayerState) {
  const { track, ...rest } = state;
  Object.assign(player, rest);
  if (track?.id !== player.track?.id) player.track = track;
}

/**
 * Keeps `player` in sync with `player://state` and loads the current state. An event that
 * arrives while the initial `player_state` is in flight is newer, so it wins. Returns a
 * function that stops listening.
 */
export async function initPlayer(): Promise<() => void> {
  let fresh = false;
  const unlisten = await onPlayerState((state) => {
    fresh = true;
    apply(state);
  });
  try {
    const state = await playerState();
    if (!fresh) apply(state);
  } catch (err) {
    console.error('loading the player state failed', err);
  }
  return unlisten;
}
