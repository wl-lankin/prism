<script lang="ts" module>
  /** What a view keeps while its track menu is open (everything but `onclose`). */
  export interface TrackMenuState {
    trackIds: string[];
    x: number;
    y: number;
    albumId?: string;
    artist?: string;
    album?: boolean;
    onremove?: () => void;
  }

  /** Keeps menus off the window edges and out of the transparent strip below the card. */
  const EDGE = 8;

  /** The area a menu must stay inside: the element App marks with `data-menu-bounds` (the
   *  views' area, clear of the transport strip and the Flow ring), else the window. */
  function bounds(): { left: number; top: number; right: number; bottom: number } {
    const area = document.querySelector('[data-menu-bounds]')?.getBoundingClientRect();
    if (area && area.width > 0 && area.height > 0) return area;
    return { left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight };
  }
</script>

<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { t, tc } from '../i18n';
  import { isFavorite, setFavorite } from '../stores/favorites.svelte';
  import { go } from '../stores/nav.svelte';
  import { addToPlaylist, createWithTracks, playlists, suggestName } from '../stores/playlists.svelte';
  import type { Playlist } from '../types';
  import Cover from './Cover.svelte';
  import Icon from './Icon.svelte';

  /** The track context menu: add to a playlist (or a new one, named in place), favorite a single
   *  track, go to its album or artist, and, inside a playlist, remove the entry. A menu in the
   *  WAI-ARIA sense: focus moves with the arrows, Right opens the playlists, Left and Esc step
   *  back, a press anywhere else closes it. */
  let {
    trackIds,
    x,
    y,
    albumId,
    artist,
    album = false,
    onremove,
    onclose,
  }: TrackMenuState & { onclose: () => void } = $props();

  let root = $state<HTMLDivElement>();
  // Placed where it was opened; onMount then keeps it inside the card.
  let left = $state(untrack(() => x));
  let top = $state(untrack(() => y));
  let subOpen = $state(false);
  let creating = $state(false);
  let draft = $state('');
  let busy = $state(false);
  let parentItem = $state<HTMLButtonElement>();

  const single = $derived(!album && trackIds.length === 1 ? trackIds[0] : null);
  const favorite = $derived(single ? isFavorite('track', single) : false);
  const list = $derived(playlists.list ?? []);

  onMount(() => {
    const opener = document.activeElement as HTMLElement | null;
    if (root) {
      const box = root.getBoundingClientRect();
      const area = bounds();
      left = Math.max(area.left + EDGE, Math.min(x, area.right - box.width - EDGE));
      top = y + box.height > area.bottom - EDGE ? Math.max(area.top + EDGE, y - box.height) : y;
      items(root)[0]?.focus({ preventScroll: true });
    }
    return () => {
      // Back to where the menu was opened from, unless focus has moved on meanwhile.
      const now = document.activeElement;
      if (opener?.isConnected && (!now || now === document.body || root?.contains(now))) {
        opener.focus({ preventScroll: true });
      }
    };
  });

  /** The enabled items of one menu level (not those of the submenu inside it). */
  function items(menu: Element): HTMLElement[] {
    return [...menu.querySelectorAll<HTMLElement>('[role="menuitem"]')].filter(
      (el) => el.closest('[role="menu"]') === menu,
    );
  }

  function close() {
    onclose();
  }

  async function openSub(focusFirst: boolean) {
    subOpen = true;
    await tick();
    if (focusFirst) {
      const sub = root?.querySelector('.sub');
      if (sub) items(sub)[0]?.focus({ preventScroll: true });
    }
  }

  function closeSub(refocus: boolean) {
    subOpen = false;
    creating = false;
    if (refocus) parentItem?.focus({ preventScroll: true });
  }

  /** The pointer moved on to another item: the playlists close, unless a name is being typed. */
  function leaveSub() {
    if (subOpen && !creating) closeSub(false);
  }

  /** Flips the playlists to the left when they would leave the card, and lifts them when
   *  they would run past its bottom edge. */
  function placeSub(node: HTMLElement) {
    const area = bounds();
    const parent = node.parentElement!.getBoundingClientRect();
    const box = node.getBoundingClientRect();
    node.classList.toggle('flip', parent.right + box.width + 4 > area.right - EDGE);
    const overflow = box.bottom - (area.bottom - EDGE);
    if (overflow > 0) node.style.top = `${-6 - overflow}px`;
  }

  async function choose(playlist: Playlist) {
    if (busy) return;
    busy = true;
    await addToPlaylist(playlist, trackIds);
    busy = false;
    close();
  }

  async function startCreating() {
    draft = suggestName();
    creating = true;
    await tick();
    const input = root?.querySelector<HTMLInputElement>('.sub input');
    input?.focus();
    input?.select();
  }

  async function create() {
    const name = draft.trim();
    if (!name || busy) return;
    busy = true;
    await createWithTracks(name, trackIds);
    busy = false;
    close();
  }

  function oninputkey(event: KeyboardEvent) {
    // Tab leaves the field like any menu item: the menu's own handler closes the menu.
    if (event.key === 'Tab') return;
    // Typing stays in the field; only Enter and Esc act.
    event.stopPropagation();
    if (event.key === 'Enter') {
      event.preventDefault();
      void create();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      creating = false;
      tick().then(() => root?.querySelector<HTMLElement>('.sub .new')?.focus({ preventScroll: true }));
    }
  }

  function toggleFavorite() {
    if (single) void setFavorite('track', single, !favorite);
    close();
  }

  function goTo(view: 'album' | 'artist', param: string) {
    close();
    go(view, param);
  }

  function remove() {
    close();
    onremove?.();
  }

  function onkeydown(event: KeyboardEvent) {
    const current = document.activeElement as HTMLElement | null;
    const menu = current?.closest('[role="menu"]') ?? root;
    if (!menu) return;
    const inSub = menu !== root;
    const all = items(menu);
    const i = current ? all.indexOf(current) : -1;
    const moves: Record<string, number> = {
      ArrowDown: (i + 1) % all.length,
      ArrowUp: (i - 1 + all.length) % all.length,
      Home: 0,
      End: all.length - 1,
    };
    if (event.key in moves) {
      event.preventDefault();
      all[moves[event.key]]?.focus({ preventScroll: true });
    } else if (event.key === 'ArrowRight' && !inSub && current === parentItem) {
      event.preventDefault();
      void openSub(true);
    } else if (event.key === 'ArrowLeft' && inSub) {
      event.preventDefault();
      closeSub(true);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      if (inSub) closeSub(true);
      else close();
    } else if (event.key === 'Tab') {
      event.preventDefault();
      close();
    }
  }

  function onwindowpointerdown(event: PointerEvent) {
    if (root && !root.contains(event.target as Node | null)) close();
  }

  /** Pointer and keyboard share one highlight: hovering an item focuses it. */
  function follow(event: PointerEvent) {
    const item = (event.target as HTMLElement).closest<HTMLElement>('[role="menuitem"]');
    if (item && item !== document.activeElement && !creating) item.focus({ preventScroll: true });
  }
</script>

<svelte:window onpointerdown={onwindowpointerdown} onblur={close} onresize={close} onwheel={(event) => {
  if (!root?.contains(event.target as Node | null)) close();
}} />

<div
  bind:this={root}
  class="menu"
  role="menu"
  tabindex="-1"
  aria-label={t('menu.label')}
  style="left:{left}px; top:{top}px"
  {onkeydown}
  onpointermove={follow}
  oncontextmenu={(event) => event.preventDefault()}
>
  <div class="has-sub" role="none">
    <button
      bind:this={parentItem}
      type="button"
      role="menuitem"
      tabindex="-1"
      class="item"
      class:open={subOpen}
      aria-haspopup="menu"
      aria-expanded={subOpen}
      onclick={(event) => openSub(event.detail === 0)}
      onpointerenter={() => !subOpen && openSub(false)}
    >
      <Icon name="playlists" size={16} />
      <span class="label">{album ? t('menu.addAlbumToPlaylist') : t('menu.addToPlaylist')}</span>
      <span class="chev"><Icon name="chevronRight" size={14} /></span>
    </button>

    {#if subOpen}
      <div class="sub menu" role="menu" tabindex="-1" aria-label={t('menu.addToPlaylist')} {@attach placeSub}>
        {#if creating}
          <div class="field" role="none">
            <span class="plus" aria-hidden="true"><Icon name="plus" size={14} /></span>
            <input
              type="text"
              bind:value={draft}
              maxlength="200"
              aria-label={t('playlists.nameLabel')}
              spellcheck="false"
              disabled={busy}
              onkeydown={oninputkey}
            />
            <button
              type="button"
              class="ok"
              aria-label={t('playlists.new')}
              disabled={!draft.trim() || busy}
              onclick={create}
            >
              <Icon name="check" size={14} />
            </button>
          </div>
        {:else}
          <button type="button" role="menuitem" tabindex="-1" class="item new" onclick={startCreating}>
            <span class="plus" aria-hidden="true"><Icon name="plus" size={14} /></span>
            <span class="label">{t('playlists.newMenu')}</span>
          </button>
        {/if}
        {#if list.length}
          <div class="sep" role="separator"></div>
          <div class="scroll" role="none">
            {#each list as playlist (playlist.id)}
              <button
                type="button"
                role="menuitem"
                tabindex="-1"
                class="item"
                disabled={busy}
                onclick={() => choose(playlist)}
              >
                <span class="thumb" aria-hidden="true">
                  <Cover coverId={playlist.coverIds[0] ?? null} size={22} cache={96} radius="5px" />
                </span>
                <span class="label">{playlist.name}</span>
                <span class="count" aria-hidden="true">{tc('view.count.songs', playlist.trackCount)}</span>
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </div>

  {#if single}
    <button
      type="button"
      role="menuitem"
      tabindex="-1"
      class="item"
      class:fav={favorite}
      onclick={toggleFavorite}
      onpointerenter={leaveSub}
    >
      <Icon name="heart" size={16} filled={favorite} />
      <span class="label">{favorite ? t('favorites.remove') : t('favorites.add')}</span>
    </button>
  {/if}

  {#if albumId || artist}
    <div class="sep" role="separator"></div>
    {#if albumId}
      <button
        type="button"
        role="menuitem"
        tabindex="-1"
        class="item"
        onclick={() => goTo('album', albumId)}
        onpointerenter={leaveSub}
      >
        <Icon name="album" size={16} />
        <span class="label">{t('menu.goToAlbum')}</span>
      </button>
    {/if}
    {#if artist}
      <button
        type="button"
        role="menuitem"
        tabindex="-1"
        class="item"
        onclick={() => goTo('artist', artist)}
        onpointerenter={leaveSub}
      >
        <Icon name="artist" size={16} />
        <span class="label">{t('menu.goToArtist')}</span>
      </button>
    {/if}
  {/if}

  {#if onremove}
    <div class="sep" role="separator"></div>
    <button
      type="button"
      role="menuitem"
      tabindex="-1"
      class="item danger"
      aria-keyshortcuts="Delete"
      onclick={remove}
      onpointerenter={leaveSub}
    >
      <Icon name="remove" size={16} />
      <span class="label">{t('menu.removeFromPlaylist')}</span>
    </button>
  {/if}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 40;
    min-width: 224px;
    max-width: 300px;
    padding: 5px;
    box-sizing: border-box;
    border-radius: 12px;
    background: rgba(31, 31, 37, 0.985);
    box-shadow:
      inset 0 0 0 1px rgba(255, 255, 255, 0.08),
      0 1px 0 0 rgba(255, 255, 255, 0.04) inset,
      0 22px 50px -14px rgba(0, 0, 0, 0.8),
      0 6px 16px -6px rgba(0, 0, 0, 0.5);
    font-size: 13px;
    color: var(--text);
    outline: none;
    transform-origin: top left;
    animation: pop var(--dur) ease-out;
    user-select: none;
    -webkit-user-select: none;
  }

  .has-sub {
    position: relative;
  }

  .sub {
    position: absolute;
    top: -6px;
    left: calc(100% + 6px);
    width: 252px;
    max-width: none;
    animation-name: pop-side;
  }

  /* Flipped to the left of the menu when the right side has no room. */
  .sub:global(.flip) {
    left: auto;
    right: calc(100% + 6px);
    transform-origin: top right;
  }

  .scroll {
    max-height: 280px;
    overflow-y: auto;
    overflow-x: hidden;
    margin-right: -3px;
    padding-right: 3px;
  }

  .item {
    width: 100%;
    height: 32px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 10px 0 9px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: rgba(243, 243, 245, 0.86);
    font: inherit;
    text-align: left;
    cursor: default;
    outline: none;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .item :global(.icon) {
    color: rgba(243, 243, 245, 0.55);
    transition: color var(--dur) ease-out;
  }

  .item:focus,
  .item.open {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
  }

  .item:focus :global(.icon),
  .item.open :global(.icon) {
    color: var(--text);
  }

  .item:disabled {
    opacity: 0.5;
  }

  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .chev {
    display: grid;
    margin-right: -4px;
  }

  .item.fav :global(.icon) {
    color: var(--spec-1);
  }

  .danger:focus {
    background: color-mix(in srgb, var(--spec-1) 16%, transparent);
  }

  .danger:focus :global(.icon) {
    color: var(--spec-1);
  }

  .count {
    flex: none;
    font-size: 11.5px;
    color: rgba(243, 243, 245, 0.4);
    font-variant-numeric: tabular-nums;
  }

  .thumb {
    flex: none;
    display: grid;
    border-radius: 5px;
    box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  /* "New playlist...": a dashed tile where the covers are, so it reads as a slot to fill. */
  .plus {
    flex: none;
    width: 22px;
    height: 22px;
    display: grid;
    place-items: center;
    border-radius: 5px;
    box-shadow: inset 0 0 0 1px rgba(243, 243, 245, 0.22);
    background: rgba(255, 255, 255, 0.03);
  }

  .new:focus .plus {
    box-shadow: inset 0 0 0 1px rgba(243, 243, 245, 0.5);
  }

  /* The name field takes the item's place, at the item's height. */
  .field {
    height: 32px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 3px 0 9px;
    border-radius: 7px;
    background: rgba(0, 0, 0, 0.28);
    box-shadow: inset 0 0 0 1px rgba(77, 166, 255, 0.55);
  }

  .field input {
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--text);
    font: inherit;
    outline: none;
    user-select: text;
    -webkit-user-select: text;
  }

  .field .plus :global(.icon) {
    color: var(--spec-4);
  }

  .ok {
    width: 26px;
    height: 26px;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 6px;
    background: var(--text);
    color: var(--bg);
    cursor: pointer;
    transition: opacity var(--dur) ease-out;
  }

  .ok:disabled {
    opacity: 0.3;
    cursor: default;
  }

  .ok:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 1px;
  }

  .sep {
    height: 1px;
    margin: 5px 8px;
    background: var(--line);
  }

  @keyframes pop {
    from {
      opacity: 0;
      transform: scale(0.97);
    }
  }

  @keyframes pop-side {
    from {
      opacity: 0;
      transform: translateX(-4px);
    }
  }
</style>
