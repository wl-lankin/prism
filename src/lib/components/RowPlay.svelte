<script lang="ts">
  import { t } from '../i18n';
  import { playerToggle } from '../ipc';
  import { notify } from '../stores/notice.svelte';
  import { player } from '../stores/player.svelte';
  import Icon from './Icon.svelte';
  import NowPlayingBars from './NowPlayingBars.svelte';

  /** The number cell of a song row. It shows the row's number, or the equalizer on the row
   *  that plays. While the pointer is over the row (or the row is selected), the number turns
   *  into a play button; on the playing row, the equalizer turns into pause or play only while
   *  the pointer is over it, so the marker stays visible on a selected row. The button is out
   *  of the tab order like the row's heart: the list's own Enter key plays the selected row.
   *  A click does not focus it either (mousedown is cancelled), so focus stays on the list and
   *  a later Enter acts once, on the list's selection. */
  let {
    number,
    name,
    current = false,
    revealed = false,
    suppressed = false,
    onplay,
  }: {
    number: number | string;
    /** The track's title, for the button's accessible name. */
    name: string;
    /** This row is the track that plays (or is paused) now. */
    current?: boolean;
    /** The row is selected. */
    revealed?: boolean;
    /** Keeps the button hidden, for example while rows are dragged. */
    suppressed?: boolean;
    /** Plays this row, as a double-click would. */
    onplay: () => void;
  } = $props();

  let hovered = $state(false);

  const playing = $derived(player.status === 'playing');
  const shown = $derived(!suppressed && (current ? hovered : hovered || revealed));
  const pauses = $derived(current && playing);
  const label = $derived(
    pauses ? t('row.pause', { name }) : current ? t('row.resume', { name }) : t('row.play', { name }),
  );

  /** Follows the pointer over the whole row this cell sits in. */
  function followRow(node: HTMLElement) {
    const row = node.closest<HTMLElement>('[data-index]');
    if (!row) return;
    const enter = () => (hovered = true);
    const leave = () => (hovered = false);
    row.addEventListener('mouseenter', enter);
    row.addEventListener('mouseleave', leave);
    return () => {
      row.removeEventListener('mouseenter', enter);
      row.removeEventListener('mouseleave', leave);
    };
  }

  function onclick(event: MouseEvent) {
    event.stopPropagation();
    if (current) {
      playerToggle().catch((err) => notify(t('flow.failed', { message: String(err) })));
    } else {
      onplay();
    }
  }
</script>

<span class="rowplay" class:shown {@attach followRow}>
  <span class="mark" aria-hidden={shown ? 'true' : undefined}>
    {#if current}
      <NowPlayingBars {playing} size={12} />
    {:else}
      {number}
    {/if}
  </span>
  <button
    type="button"
    class="play"
    tabindex="-1"
    aria-label={label}
    title={label}
    aria-hidden={shown ? undefined : 'true'}
    {onclick}
    ondblclick={(event) => event.stopPropagation()}
    onpointerdown={(event) => event.stopPropagation()}
    onmousedown={(event) => event.preventDefault()}
  >
    <Icon name={pauses ? 'pause' : 'play'} size={15} filled />
  </button>
</span>

<style>
  .rowplay {
    position: relative;
    display: inline-grid;
    place-items: center end;
    min-width: 28px;
    height: 28px;
  }

  .mark,
  .play {
    grid-area: 1 / 1;
    transition: opacity var(--dur) ease-out;
  }

  .mark {
    display: flex;
    align-items: center;
  }

  /* Same footprint as the number's column end, so nothing shifts when it swaps in. */
  .play {
    width: 28px;
    height: 28px;
    display: grid;
    place-items: center;
    margin-right: -7px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--text);
    cursor: pointer;
    opacity: 0;
    pointer-events: none;
    transition:
      opacity var(--dur) ease-out,
      background-color var(--dur) ease-out,
      transform var(--dur) ease-out;
  }

  .shown .mark {
    opacity: 0;
  }

  .shown .play {
    opacity: 1;
    pointer-events: auto;
  }

  .play:hover {
    background: rgba(255, 255, 255, 0.1);
  }

  .play:active {
    transform: scale(0.92);
  }

  .play:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 0;
  }
</style>
