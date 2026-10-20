<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { playFlow, sameFlow } from '../flow';
  import { t } from '../i18n';
  import { libraryGenres } from '../ipc';
  import { player } from '../stores/player.svelte';
  import type { FlowVariant } from '../types';
  import Icon from './Icon.svelte';

  /** The Flow variants, opened at a point (above the ring, after a right-click or long
   *  press). Picking one closes the menu and starts that Flow through `playFlow`, or hands
   *  the variant to `onpick` instead (the mini player handles an empty library and the
   *  notice itself). Esc, a click elsewhere, focus leaving it or leaving the window closes
   *  it; focus then returns to what had it before (the ring). Used by the main window and
   *  the mini player. */
  let {
    x,
    y,
    onclose,
    onpick,
  }: { x: number; y: number; onclose: () => void; onpick?: (variant: FlowVariant) => void } = $props();

  const MARGIN = 8;

  let menu: HTMLDivElement | undefined = $state();
  let filterInput: HTMLInputElement | undefined = $state();
  let left = $state(0);
  /** Opening above: anchored by its bottom edge, so a list that shrinks (filtering) stays
   *  next to the point. Below: anchored by its top edge. */
  let edge: { side: 'top' | 'bottom'; px: number } = $state({ side: 'bottom', px: 0 });
  let room = $state(0);
  let placed = $state(false);
  /** Whatever had focus when the menu opened; it gets it back on close. */
  const opener = typeof document !== 'undefined' ? (document.activeElement as HTMLElement | null) : null;
  let picking = $state(false);
  let genres: string[] | null = $state(null);
  let genreError: string | null = $state(null);
  let filter = $state('');

  const shown = $derived.by(() => {
    const needle = filter.trim().toLowerCase();
    return (genres ?? []).filter((genre) => genre.toLowerCase().includes(needle));
  });
  const playing = $derived(player.status === 'idle' ? null : player.flow);

  /** Centred on the point, above it when it fits there or has more room than below; never
   *  past the window's edges (a short window caps the height and the genre list scrolls). */
  async function place() {
    await tick();
    if (!menu) return;
    const { width, height } = menu.getBoundingClientRect();
    const above = y - 2 * MARGIN;
    const below = window.innerHeight - y - 2 * MARGIN;
    if (height <= above || above >= below) {
      edge = { side: 'bottom', px: window.innerHeight - y + MARGIN };
      room = Math.max(0, above);
    } else {
      edge = { side: 'top', px: y + MARGIN };
      room = Math.max(0, below);
    }
    left = Math.max(MARGIN, Math.min(x - width / 2, window.innerWidth - width - MARGIN));
    placed = true;
  }

  onMount(() => {
    void place().then(() => items()[0]?.focus());
    return () => {
      // Back to the ring, unless the user already moved focus somewhere else.
      const active = document.activeElement;
      if (opener?.isConnected && (!active || active === document.body || menu?.contains(active))) opener.focus();
    };
  });

  /** Keeps focus inside while the content swaps (the focused item is about to go away). */
  function holdFocus() {
    menu?.focus();
  }

  function onfocusout(event: FocusEvent) {
    const next = event.relatedTarget as Node | null;
    if (next && !menu?.contains(next)) onclose();
  }

  $effect(() => {
    // Re-place when opened at another point (a second right-click).
    void x;
    void y;
    void place();
  });

  function items(): HTMLElement[] {
    return menu ? [...menu.querySelectorAll<HTMLElement>('[role="menuitem"]')] : [];
  }

  async function start(variant: FlowVariant) {
    onclose();
    if (onpick) onpick(variant);
    else await playFlow(variant);
  }

  async function openGenres() {
    holdFocus();
    picking = true;
    filter = '';
    if (genres === null) {
      try {
        genres = await libraryGenres();
        genreError = null;
      } catch (err) {
        genres = [];
        genreError = String(err);
      }
    }
    await place();
    filterInput?.focus();
  }

  async function back() {
    holdFocus();
    picking = false;
    await place();
    items()[2]?.focus();
  }

  function onkeydown(event: KeyboardEvent) {
    const list = items();
    const at = list.indexOf(document.activeElement as HTMLElement);
    switch (event.key) {
      case 'Escape':
        event.preventDefault();
        event.stopPropagation();
        onclose();
        break;
      case 'ArrowDown':
        event.preventDefault();
        list[(at + 1) % list.length]?.focus();
        break;
      case 'ArrowUp':
        event.preventDefault();
        if (picking && at <= 0) filterInput?.focus();
        else list[(at - 1 + list.length) % list.length]?.focus();
        break;
      case 'Home':
        if (document.activeElement === filterInput) return;
        event.preventDefault();
        list[0]?.focus();
        break;
      case 'End':
        if (document.activeElement === filterInput) return;
        event.preventDefault();
        list[list.length - 1]?.focus();
        break;
      case 'Enter':
        // Enter in the filter picks the first match.
        if (document.activeElement === filterInput && shown.length > 0) {
          event.preventDefault();
          void start({ kind: 'genre', genre: shown[0] });
        }
        break;
      default:
        // Typing anywhere in the genre list goes to the filter.
        if (picking && event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
          if (document.activeElement !== filterInput) filterInput?.focus();
        }
    }
  }

  function onwindowpointerdown(event: PointerEvent) {
    if (menu && !menu.contains(event.target as Node | null)) onclose();
  }
</script>

<svelte:window onpointerdown={onwindowpointerdown} onblur={onclose} onresize={onclose} />

<div
  bind:this={menu}
  class="menu"
  class:placed
  style="left:{left}px;{edge.side}:{edge.px}px;max-height:{room}px"
  role="menu"
  tabindex="-1"
  aria-label={picking ? t('flow.fromGenre') : t('flow.menu')}
  {onkeydown}
  {onfocusout}
  oncontextmenu={(event) => event.preventDefault()}
>
  {#if !picking}
    <button
      type="button"
      role="menuitem"
      class="item"
      class:on={sameFlow(playing, { kind: 'favorites' })}
      onclick={() => start({ kind: 'favorites' })}
    >
      <span class="icon fav"><Icon name="heart" size={17} /></span>
      <span class="text">{t('flow.fromFavorites')}</span>
    </button>
    <button
      type="button"
      role="menuitem"
      class="item"
      class:on={sameFlow(playing, { kind: 'library' })}
      onclick={() => start({ kind: 'library' })}
    >
      <span class="icon lib"><Icon name="album" size={17} /></span>
      <span class="text">{t('flow.fromLibrary')}</span>
    </button>
    <button
      type="button"
      role="menuitem"
      class="item"
      class:on={playing?.kind === 'genre'}
      aria-haspopup="menu"
      onclick={openGenres}
    >
      <span class="icon genre"><Icon name="songs" size={17} /></span>
      <span class="text">
        {t('flow.fromGenre')}
        {#if playing?.kind === 'genre'}<span class="current">{playing.genre}</span>{/if}
      </span>
      <span class="more"><Icon name="back" size={15} /></span>
    </button>
  {:else}
    <div class="head">
      <button type="button" class="back" aria-label={t('flow.back')} onclick={back}>
        <Icon name="back" size={16} />
      </button>
      <input
        bind:this={filterInput}
        bind:value={filter}
        class="filter"
        type="text"
        spellcheck="false"
        autocomplete="off"
        placeholder={t('flow.genreFilter')}
        aria-label={t('flow.genreFilter')}
      />
    </div>
    <div class="genres">
      {#if genres === null}
        <!-- Loading; it takes a moment at most. -->
      {:else if genreError}
        <p class="empty">{t('flow.genresFailed', { message: genreError })}</p>
      {:else if genres.length === 0}
        <p class="empty">{t('flow.noGenres')}</p>
      {:else if shown.length === 0}
        <p class="empty">{t('flow.noGenreMatch')}</p>
      {:else}
        {#each shown as genre (genre)}
          <button
            type="button"
            role="menuitem"
            class="item genre-item"
            class:on={sameFlow(playing, { kind: 'genre', genre })}
            onclick={() => start({ kind: 'genre', genre })}
          >
            <span class="text">{genre}</span>
          </button>
        {/each}
      {/if}
    </div>
  {/if}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 40;
    width: 300px;
    max-width: calc(100vw - 16px);
    box-sizing: border-box;
    padding: 5px;
    border-radius: 14px;
    background: var(--raised);
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.09),
      0 22px 48px -10px rgba(0, 0, 0, 0.75),
      0 4px 14px rgba(0, 0, 0, 0.4);
    color: var(--text);
    visibility: hidden;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .menu.placed {
    visibility: visible;
    animation: rise var(--dur) ease-out;
  }

  .menu:focus {
    outline: none;
  }

  .item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 38px;
    padding: 6px 10px 6px 8px;
    border: 0;
    border-radius: 9px;
    background: transparent;
    color: inherit;
    font: inherit;
    font-size: 13.5px;
    text-align: left;
    cursor: pointer;
    transition: background-color var(--dur) ease-out;
  }

  .item:hover,
  .item:focus-visible {
    background: rgba(255, 255, 255, 0.08);
    outline: none;
  }

  .item.on .text {
    font-weight: 600;
  }

  .icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.05);
  }

  .icon.fav {
    color: var(--spec-1);
  }

  .icon.lib {
    color: var(--spec-4);
  }

  .icon.genre {
    color: var(--spec-2);
  }

  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .current {
    font-size: 11.5px;
    font-weight: 400;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .more {
    flex: none;
    display: grid;
    color: var(--text-dim);
    transform: rotate(180deg);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 2px 6px;
    border-bottom: 1px solid var(--line);
    margin-bottom: 4px;
  }

  .back {
    flex: none;
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--text-dim);
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .back:hover,
  .back:focus-visible {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
    outline: none;
  }

  .filter {
    flex: 1;
    min-width: 0;
    height: 30px;
    padding: 0 10px;
    border: 0;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.055);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.07);
    color: var(--text);
    font: inherit;
    font-size: 13px;
  }

  .filter:focus {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--spec-4);
  }

  .genres {
    flex: 0 1 auto;
    min-height: 0;
    max-height: 240px;
    overflow-y: auto;
  }

  .item,
  .head {
    flex: none;
  }

  .genre-item {
    min-height: 32px;
    padding-left: 12px;
  }

  .genre-item.on::before {
    content: '';
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--spec-3);
  }

  .empty {
    margin: 0;
    padding: 10px 12px;
    font-size: 12.5px;
    color: var(--text-dim);
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(3px);
    }
  }
</style>
