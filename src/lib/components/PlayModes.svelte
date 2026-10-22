<script lang="ts">
  import { lang } from '../i18n';
  import { cycleRepeat, repeatLabel, shuffleLabel, toggleShuffle } from '../playmodes';
  import { player } from '../stores/player.svelte';
  import Icon from './Icon.svelte';

  /** Shuffle and repeat, side by side, for the transport strip and (`compact`) the mini
   *  player. A mode that is on lights up with a short spectrum bar under its icon. In Flow
   *  both stay visible but unavailable, and their tooltips say why. */
  let { compact = false }: { compact?: boolean } = $props();

  const flow = $derived(!!player.flow);
  const repeat = $derived(player.repeat ?? 'off');
  const shuffle = $derived(!!player.shuffle);
  const iconSize = $derived(compact ? 15 : 18);
  // Read so the labels follow a language change.
  const labels = $derived.by(() => {
    void lang();
    return { repeat: repeatLabel(repeat, flow), shuffle: shuffleLabel(shuffle, flow) };
  });

  function report(err: unknown) {
    console.error('changing the play mode failed', err);
  }
</script>

<div class="modes" class:compact>
  <button
    type="button"
    class="mode"
    class:on={shuffle && !flow}
    aria-pressed={shuffle && !flow}
    aria-disabled={flow}
    aria-label={labels.shuffle}
    title={labels.shuffle}
    onclick={() => {
      if (!flow) toggleShuffle().catch(report);
    }}
  >
    <Icon name="shuffle" size={iconSize} />
  </button>
  <button
    type="button"
    class="mode"
    class:on={repeat !== 'off' && !flow}
    aria-pressed={repeat !== 'off' && !flow}
    aria-disabled={flow}
    aria-label={labels.repeat}
    title={labels.repeat}
    onclick={() => {
      if (!flow) cycleRepeat().catch(report);
    }}
  >
    <Icon name={repeat === 'one' ? 'repeatOne' : 'repeat'} size={iconSize} />
  </button>
</div>

<style>
  .modes {
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .mode {
    position: relative;
    flex: none;
    width: 34px;
    height: 34px;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: rgba(243, 243, 245, 0.62);
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out,
      transform var(--dur) ease-out;
  }

  .compact {
    gap: 0;
  }

  .compact .mode {
    width: 23px;
    height: 26px;
  }

  .mode:hover:not([aria-disabled='true']) {
    background: rgba(255, 255, 255, 0.07);
    color: var(--text);
  }

  .mode:active:not([aria-disabled='true']) {
    transform: scale(0.94);
  }

  .mode.on {
    color: var(--text);
  }

  /* A mode that is on carries a sliver of the prism's light under its icon. */
  .mode::after {
    content: '';
    position: absolute;
    left: 50%;
    bottom: 4px;
    width: 12px;
    height: 2px;
    border-radius: 2px;
    background: linear-gradient(90deg, var(--spec-1), var(--spec-2), var(--spec-3), var(--spec-4));
    opacity: 0;
    transform: translateX(-50%) scaleX(0.4);
    transition:
      opacity var(--dur) ease-out,
      transform var(--dur) ease-out;
  }

  .compact .mode::after {
    bottom: 2px;
    width: 10px;
  }

  .mode.on::after {
    opacity: 1;
    transform: translateX(-50%) scaleX(1);
  }

  .mode[aria-disabled='true'] {
    color: rgba(243, 243, 245, 0.22);
    cursor: default;
  }

  .mode:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 0;
  }
</style>
