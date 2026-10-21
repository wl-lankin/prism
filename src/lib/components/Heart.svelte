<script lang="ts">
  import { t } from '../i18n';
  import { favorites, heartable, isFavorite, setFavorite } from '../stores/favorites.svelte';
  import { notify } from '../stores/notice.svelte';
  import { ensureStation, findStation, toggleStationFavorite } from '../stores/radio.svelte';
  import type { FavKind } from '../types';
  import Icon from './Icon.svelte';

  /** The favorite toggle. Off it is an outline; on it fills with the first spectrum color, and
   *  turning it on sends a short prism-colored ring out from it.
   *
   *  `id` may name several items that stand for one thing (an artist found in more than one
   *  source): the heart is on when any of them is a favorite, and a click sets all of them to
   *  the new state. Navidrome ids drop out while no account is signed in (the backend refuses
   *  them); a heart left with nothing to change is `aria-disabled`, dimmed, and a click on it
   *  says why instead of changing anything. */
  let {
    kind,
    id,
    size = 18,
    label,
    round = false,
    tabindex,
    ghost = false,
    revealed = false,
  }: {
    kind: FavKind;
    id: string | readonly string[];
    size?: number;
    /** The accessible name; it stays the same in both states (`aria-pressed` carries the state). */
    label?: string;
    /** A round button with a quiet fill, to sit next to the Play and Shuffle pills. */
    round?: boolean;
    /** -1 inside list rows that move with the arrow keys. */
    tabindex?: number;
    /** In a list row: while not set, hidden (no pointer, no screen reader) until the row is
     *  hovered or `revealed` (selected). Rows toggle it with the "f" key instead. */
    ghost?: boolean;
    revealed?: boolean;
  } = $props();

  let focused = $state(false);

  const ids = $derived(typeof id === 'string' ? [id] : id);
  /** The ids a click may change now. */
  const usable = $derived(ids.filter((x) => heartable(x)));
  /** A station is saved with all its fields, so its heart needs the station itself. */
  const station = $derived(kind === 'station' ? findStation(ids[0]) : null);
  const disabled = $derived(kind === 'station' ? !station : usable.length === 0);

  $effect(() => {
    if (kind === 'station') ensureStation(ids[0]);
  });

  const on = $derived.by(() => {
    // Read the set itself so a reload of the whole set is followed too.
    void favorites.version;
    return ids.some((x) => isFavorite(kind, x));
  });

  /** A focused heart never hides, so a click that un-sets it does not leave focus on nothing. */
  const hidden = $derived(ghost && !on && !revealed && !focused);

  /** Counts the times it was switched on here; each one replays the ring. */
  let bursts = $state(0);

  function toggle(event: MouseEvent) {
    event.stopPropagation();
    if (kind === 'station') {
      if (!station) return;
      if (!on) bursts++;
      void toggleStationFavorite(station);
      return;
    }
    if (disabled) {
      notify(t('navidrome.notConnected'));
      return;
    }
    const next = !on;
    if (next) bursts++;
    // Only the items not already in the new state, so nothing is queued for nothing.
    for (const x of usable) if (isFavorite(kind, x) !== next) void setFavorite(kind, x, next);
  }
</script>

<button
  type="button"
  class="heart"
  class:on
  class:round
  class:hidden
  class:muted={disabled}
  aria-disabled={disabled ? 'true' : undefined}
  aria-hidden={hidden ? 'true' : undefined}
  aria-pressed={on}
  aria-label={label ?? t('favorites.toggle')}
  title={kind === 'station' ? (disabled ? undefined : on ? t('favorites.remove') : t('favorites.add')) : disabled ? t('navidrome.notConnected') : on ? t('favorites.remove') : t('favorites.add')}
  {tabindex}
  onclick={toggle}
  onfocus={() => (focused = true)}
  onblur={() => (focused = false)}
  ondblclick={(event) => event.stopPropagation()}
  onmousedown={(event) => {
    // In a list row (out of the tab order) a click leaves focus on the list, so a later
    // Enter acts once, on the list's selection, not also on this heart.
    if (tabindex === -1) event.preventDefault();
  }}
  style="--size:{size}px"
>
  {#key bursts}
    <span class="glyph" class:pop={bursts > 0 && on}>
      <Icon name="heart" {size} filled={on} />
    </span>
    {#if bursts > 0 && on}
      <span class="burst" aria-hidden="true"></span>
    {/if}
  {/key}
</button>

<style>
  .heart {
    position: relative;
    isolation: isolate;
    flex: none;
    width: calc(var(--size) + 14px);
    height: calc(var(--size) + 14px);
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: rgba(243, 243, 245, 0.55);
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out,
      opacity var(--dur) ease-out;
  }

  /* Rows reveal it on hover with `.row:hover :global(.heart.hidden)`. */
  .hidden {
    opacity: 0;
    pointer-events: none;
  }

  .heart:hover:not(.muted) {
    color: var(--text);
    background: rgba(255, 255, 255, 0.07);
  }

  .heart.on {
    color: var(--spec-1);
  }

  .heart.on:hover:not(.muted) {
    color: color-mix(in srgb, var(--spec-1) 82%, #fff);
  }

  .heart:active:not(.muted) .glyph {
    transform: scale(0.88);
  }

  .heart:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 0;
  }

  .round {
    width: 38px;
    height: 38px;
    background: rgba(255, 255, 255, 0.06);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.1);
    color: var(--text);
  }

  .round:hover:not(.muted) {
    background: rgba(255, 255, 255, 0.1);
  }

  .round.on {
    background: color-mix(in srgb, var(--spec-1) 14%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--spec-1) 38%, transparent);
  }

  .round.on:hover:not(.muted) {
    background: color-mix(in srgb, var(--spec-1) 20%, transparent);
  }

  /* Signed out of Navidrome: it still shows the state, quietly. Dimmed by color, not opacity,
     so the rows' hover reveal (which sets the opacity) keeps working. */
  .heart.muted {
    color: rgba(243, 243, 245, 0.26);
    cursor: default;
  }

  .heart.on.muted {
    color: color-mix(in srgb, var(--spec-1) 45%, transparent);
  }

  .round:focus-visible {
    outline-offset: 2px;
  }

  .glyph {
    display: grid;
    place-items: center;
    transition: transform var(--dur) ease-out;
  }

  .on .glyph {
    filter: drop-shadow(0 0 6px color-mix(in srgb, var(--spec-1) 45%, transparent));
  }

  /* Large motion: both switch off under reduced motion (--dur-l is 0 there). */
  .pop {
    animation: pop var(--dur-l) cubic-bezier(0.3, 1.6, 0.5, 1);
  }

  .burst {
    position: absolute;
    inset: 50% auto auto 50%;
    width: calc(var(--size) * 1.9);
    height: calc(var(--size) * 1.9);
    margin: calc(var(--size) * -0.95) 0 0 calc(var(--size) * -0.95);
    border-radius: 50%;
    pointer-events: none;
    z-index: -1;
    background: conic-gradient(var(--spec-1), var(--spec-2), var(--spec-3), var(--spec-4), var(--spec-1));
    -webkit-mask: radial-gradient(closest-side, transparent 84%, #000 86%, #000 96%, transparent 100%);
    mask: radial-gradient(closest-side, transparent 84%, #000 86%, #000 96%, transparent 100%);
    opacity: 0;
    animation: burst calc(var(--dur-l) * 1.6) cubic-bezier(0.2, 0.7, 0.3, 1);
  }

  @keyframes pop {
    0% {
      transform: scale(0.6);
    }
    60% {
      transform: scale(1.18);
    }
    100% {
      transform: scale(1);
    }
  }

  @keyframes burst {
    0% {
      opacity: 1;
      transform: scale(0.7) rotate(0deg);
    }
    100% {
      opacity: 0;
      transform: scale(1.45) rotate(70deg);
    }
  }
</style>
