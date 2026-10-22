import { onPlayerLevel } from '../ipc';
import { ringColorsFor, SPECTRUM, type RingColors } from '../palette';
import { player } from './player.svelte';

/** What the Flow ring shows besides the player state: the current cover's colors (the
 *  spectrum without one) and the loudness of what plays (0 unless playing). */
export const ring: { colors: RingColors; level: number } = $state({
  colors: SPECTRUM,
  level: 0,
});

/**
 * Follows the playing track's cover and `player://level`. Call once per window, after
 * `initPlayer()`; returns a function that stops it.
 */
export function initRing(): () => void {
  let gone = false;
  let unlisten: (() => void) | undefined;
  onPlayerLevel((level) => {
    if (player.status === 'playing') ring.level = level;
  })
    .then((stop) => (gone ? stop() : (unlisten = stop)))
    .catch((err) => console.error('listening for the player level failed', err));

  const stopEffects = $effect.root(() => {
    $effect(() => {
      const coverId = player.track?.coverId ?? null;
      let current = true;
      ringColorsFor(coverId)
        .then((colors) => {
          if (current) ring.colors = colors;
        })
        .catch(() => {
          if (current) ring.colors = SPECTRUM;
        });
      return () => {
        current = false;
      };
    });
    $effect(() => {
      // A level that arrived just before a pause must not hold the glow up.
      if (player.status !== 'playing') ring.level = 0;
    });
  });

  return () => {
    gone = true;
    unlisten?.();
    stopEffects();
  };
}
