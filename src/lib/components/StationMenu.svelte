<script lang="ts" module>
  import type { Station } from '../types';

  export interface StationMenuState {
    station: Station;
    x: number;
    y: number;
  }

  /** Only web pages are opened from a station's homepage; the directory is not trusted with
   *  `file:` or any other scheme. */
  export function isWebUrl(url: string | null | undefined): url is string {
    if (!url) return false;
    try {
      const { protocol } = new URL(url);
      return protocol === 'http:' || protocol === 'https:';
    } catch {
      return false;
    }
  }

  const EDGE = 8;

  /** The area a menu must stay inside (the views' area, clear of the transport), else the window. */
  function bounds(): { left: number; top: number; right: number; bottom: number } {
    const area = document.querySelector('[data-menu-bounds]')?.getBoundingClientRect();
    if (area && area.width > 0 && area.height > 0) return area;
    return { left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight };
  }
</script>

<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { t } from '../i18n';
  import { isFavorite } from '../stores/favorites.svelte';
  import { startStation, toggleStationFavorite } from '../stores/radio.svelte';
  import Icon from './Icon.svelte';

  /** The station context menu: play, favorite, open the homepage. A menu in the WAI-ARIA
   *  sense like the track menu: arrows move, Esc and Tab close, a press elsewhere closes. */
  let { station, x, y, onclose }: StationMenuState & { onclose: () => void } = $props();

  let root = $state<HTMLDivElement>();
  let left = $state(untrack(() => x));
  let top = $state(untrack(() => y));

  const favorite = $derived(isFavorite('station', station.id));
  const homepage = $derived(isWebUrl(station.homepage) ? station.homepage : null);

  onMount(() => {
    const opener = document.activeElement as HTMLElement | null;
    if (root) {
      const box = root.getBoundingClientRect();
      const area = bounds();
      left = Math.max(area.left + EDGE, Math.min(x, area.right - box.width - EDGE));
      top = y + box.height > area.bottom - EDGE ? Math.max(area.top + EDGE, y - box.height) : y;
      items()[0]?.focus({ preventScroll: true });
    }
    return () => {
      const now = document.activeElement;
      if (opener?.isConnected && (!now || now === document.body || root?.contains(now))) {
        opener.focus({ preventScroll: true });
      }
    };
  });

  function items(): HTMLElement[] {
    return [...(root?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
  }

  // Closing unmounts the menu and drops its props, so each action reads them first.
  function play() {
    const chosen = station;
    onclose();
    void startStation(chosen);
  }

  function toggle() {
    const chosen = station;
    onclose();
    void toggleStationFavorite(chosen);
  }

  function open() {
    const url = homepage;
    onclose();
    if (url) openUrl(url).catch((err) => console.error('opening the homepage failed', err));
  }

  function onkeydown(event: KeyboardEvent) {
    const all = items();
    const i = all.indexOf(document.activeElement as HTMLElement);
    const moves: Record<string, number> = {
      ArrowDown: (i + 1) % all.length,
      ArrowUp: (i - 1 + all.length) % all.length,
      Home: 0,
      End: all.length - 1,
    };
    if (event.key in moves) {
      event.preventDefault();
      all[moves[event.key]]?.focus({ preventScroll: true });
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      onclose();
    } else if (event.key === 'Tab') {
      event.preventDefault();
      onclose();
    }
  }

  function onwindowpointerdown(event: PointerEvent) {
    if (root && !root.contains(event.target as Node | null)) onclose();
  }

  /** Pointer and keyboard share one highlight: hovering an item focuses it. */
  function follow(event: PointerEvent) {
    const item = (event.target as HTMLElement).closest<HTMLElement>('[role="menuitem"]');
    if (item && item !== document.activeElement) item.focus({ preventScroll: true });
  }
</script>

<svelte:window
  onpointerdown={onwindowpointerdown}
  onblur={onclose}
  onresize={onclose}
  onwheel={(event) => {
    if (!root?.contains(event.target as Node | null)) onclose();
  }}
/>

<div
  bind:this={root}
  class="menu"
  role="menu"
  tabindex="-1"
  aria-label={t('radio.menu.label')}
  style="left:{left}px; top:{top}px"
  {onkeydown}
  onpointermove={follow}
  oncontextmenu={(event) => event.preventDefault()}
>
  <button type="button" role="menuitem" tabindex="-1" class="item" onclick={play}>
    <Icon name="play" size={16} />
    <span class="label">{t('radio.menu.play')}</span>
  </button>
  <button type="button" role="menuitem" tabindex="-1" class="item" class:fav={favorite} onclick={toggle}>
    <Icon name="heart" size={16} filled={favorite} />
    <span class="label">{favorite ? t('favorites.remove') : t('favorites.add')}</span>
  </button>
  {#if homepage}
    <div class="sep" role="separator"></div>
    <button type="button" role="menuitem" tabindex="-1" class="item" onclick={open}>
      <Icon name="external" size={16} />
      <span class="label">{t('radio.menu.homepage')}</span>
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

  .item:focus {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
  }

  .item:focus :global(.icon) {
    color: var(--text);
  }

  .item.fav :global(.icon) {
    color: var(--spec-1);
  }

  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
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
</style>
