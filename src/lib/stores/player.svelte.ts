import { t } from '../i18n';
import { onActivePlayer, onPlayerError, onPlayerState, playerState, transportActive } from '../ipc';
import type { ActivePlayer, PlayerError, PlayerState, Track } from '../types';
import { notice, notify } from './notice.svelte';
import { findStation } from './radio.svelte';

/** The player as the engine last reported it. Call `initPlayer()` once per window. */
export const player: PlayerState & { active: ActivePlayer } = $state({
  /** Who answers the transport; `transport://active` sets it. */
  active: 'prism',
  status: 'idle',
  track: null,
  positionMs: 0,
  durationMs: 0,
  volume: 0.8,
  queueIndex: 0,
  queueLength: 0,
  queueVersion: 0,
  flow: null,
  repeat: 'off',
  shuffle: false,
});

/**
 * Copies a new state in. The track object is only replaced when the track changes, so
 * readers of `player.track` do not re-run on every 250 ms position update.
 */
function apply(state: PlayerState) {
  const { track, ...rest } = state;
  Object.assign(player, rest);
  if (track?.id !== player.track?.id || track?.coverId !== player.track?.coverId) {
    player.track = track;
  }
  if (track) remember(track);
}

/** Titles of the tracks shown lately, so an error can name a track that is no longer shown. */
const seen = new Map<string, string>();

function remember(track: Track) {
  seen.delete(track.id);
  seen.set(track.id, track.title);
  if (seen.size > 20) seen.delete(seen.keys().next().value as string);
}

function nameOf(trackId: string): string | undefined {
  return (player.track?.id === trackId ? player.track.title : undefined) ?? seen.get(trackId) ?? findStation(trackId)?.name;
}

/**
 * Keeps `player` in sync with `player://state` and loads the current state. An event that
 * arrives while the initial `player_state` is in flight is newer, so it wins. Returns a
 * function that stops listening.
 */
export async function initPlayer(): Promise<() => void> {
  let fresh = false;
  const unlistenState = await onPlayerState((state) => {
    fresh = true;
    apply(state);
  });
  let unlistenActive: () => void = () => {};
  let activeFresh = false;
  try {
    unlistenActive = await onActivePlayer((active) => {
      activeFresh = true;
      player.active = active;
    });
  } catch (err) {
    console.error('following the active player failed', err);
  }
  // Not awaited: the player state does not wait for it. A change event that came first wins.
  transportActive().then(
    (active) => {
      if (!activeFresh && (active === 'prism' || active === 'ytm')) player.active = active;
    },
    (err) => console.error('loading the active player failed', err),
  );
  const unlisten = () => {
    unlistenState();
    unlistenActive();
  };
  try {
    const state = await playerState();
    if (!fresh) apply(state);
  } catch (err) {
    console.error('loading the player state failed', err);
  }
  return unlisten;
}

/** What the window shows for one `player://error`. `offline`: the player paused on an entry
 *  it could not reach (a Navidrome track or a station, by `source`), so one notice says why
 *  and how to go on; a second report while that notice still shows adds nothing. `track`:
 *  the entry could not be played and the player moved on (or ended); a notice names it. */
export function showPlayerError(error: PlayerError): void {
  if (!error) return;
  const station = error.source === 'radio';
  if (error.kind === 'offline') {
    const text = station
      ? t('radio.offlineNotice', { name: nameOf(error.trackId) ?? t('radio.unnamed') })
      : t('navidrome.offlineNotice');
    if (notice.text !== text) notify(text);
  } else if (error.kind === 'track') {
    const name = nameOf(error.trackId);
    const text = station
      ? t('radio.stationFailed', { name: name ?? t('radio.unnamed') })
      : t('player.trackFailed', { title: name ?? t('player.unnamed') });
    if (notice.text !== text) notify(text);
  }
}

/** Follows `player://error` in this window. Resolves with the cleanup. */
export function followPlayerErrors(): Promise<() => void> {
  return onPlayerError(showPlayerError);
}
