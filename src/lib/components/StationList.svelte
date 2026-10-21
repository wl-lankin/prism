<script lang="ts" module>
  /** Row height. Taller than a song row (44): the logo tile is 40 px and the row has two lines. */
  const ROW = 56;
  let ids = 0;
</script>

<script lang="ts">
  import { t } from '../i18n';
  import { menuPoint } from '../menu';
  import { FAVORITE_KEY, isFavoriteKey } from '../stores/favorites.svelte';
  import { player } from '../stores/player.svelte';
  import { countryName, startStation, toggleStationFavorite } from '../stores/radio.svelte';
  import type { Station } from '../types';
  import { visibleRange } from '../virtual';
  import Heart from './Heart.svelte';
  import RowPlay from './RowPlay.svelte';
  import StationMenu, { type StationMenuState } from './StationMenu.svelte';
  import StationTile from './StationTile.svelte';

  /** Station rows. With `virtual` (the default) only the rows near the viewport exist; the
   *  list follows the nearest scrolling ancestor, so a view can put a heading above it and
   *  more below. Short lists (a few saved stations) pass `virtual={false}`. */
  let {
    stations,
    showDistance = false,
    virtual = true,
  }: {
    stations: Station[];
    /** Shows "12 km" on stations that carry a distance. */
    showDistance?: boolean;
    virtual?: boolean;
  } = $props();

  const uid = ++ids;
  const rowId = (i: number) => `station-${uid}-${i}`;

  let scroller: HTMLElement | null = null;
  let scrollTop = $state(0);
  let viewport = $state(0);
  let listTop = $state(0);
  let selected = $state(-1);
  let menu = $state<StationMenuState | null>(null);
  let measure: (() => void) | undefined;

  const total = $derived(stations.length);
  const range = $derived(
    virtual ? visibleRange(scrollTop - listTop, viewport, ROW, total, 8) : { start: 0, end: total },
  );
  const rows = $derived.by(() => {
    const out: { index: number; station: Station }[] = [];
    for (let i = range.start; i < range.end; i++) out.push({ index: i, station: stations[i] });
    return out;
  });
  const playingId = $derived(player.track?.id ?? null);
  const activeId = $derived(selected >= range.start && selected < range.end ? rowId(selected) : undefined);

  /** Follows the scrolling ancestor: its offset, its height and where the list starts in it. */
  function follow(node: HTMLElement) {
    if (!virtual) return;
    let parent: HTMLElement | null = null;
    for (let el = node.parentElement; el; el = el.parentElement) {
      const overflow = getComputedStyle(el).overflowY;
      if (overflow === 'auto' || overflow === 'scroll') {
        parent = el;
        break;
      }
    }
    scroller = parent;
    if (!parent) return;
    const box = parent;
    const update = () => {
      scrollTop = box.scrollTop;
      viewport = box.clientHeight;
      listTop = node.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop;
    };
    measure = update;
    box.addEventListener('scroll', update, { passive: true });
    const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(update) : undefined;
    observer?.observe(box);
    update();
    return () => {
      box.removeEventListener('scroll', update);
      observer?.disconnect();
      measure = undefined;
      scroller = null;
    };
  }

  // Rows above the list may have changed height or the list its length.
  $effect(() => {
    void total;
    measure?.();
  });

  function reveal(i: number) {
    if (scroller) {
      const top = listTop + i * ROW;
      if (top < scroller.scrollTop) scroller.scrollTop = top;
      else if (top + ROW > scroller.scrollTop + viewport) scroller.scrollTop = top + ROW - viewport;
    } else {
      document.getElementById(rowId(i))?.scrollIntoView({ block: 'nearest' });
    }
  }

  function rowIndex(event: MouseEvent): number | null {
    if ((event.target as HTMLElement).closest('button')) return null;
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-index]');
    return row ? Number(row.dataset.index) : null;
  }

  function oncontextmenu(event: MouseEvent) {
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-index]');
    const i = row ? Number(row.dataset.index) : selected;
    const station = i >= 0 ? stations[i] : undefined;
    if (!station) return;
    event.preventDefault();
    selected = i;
    menu = { station, ...menuPoint(event, document.getElementById(rowId(i))) };
  }

  function onkeydown(event: KeyboardEvent) {
    if (total === 0) return;
    const page = Math.max(1, Math.floor(viewport / ROW) - 1);
    const moves: Record<string, number> = {
      ArrowDown: selected + 1,
      ArrowUp: selected - 1,
      PageDown: selected + page,
      PageUp: selected - page,
      Home: 0,
      End: total - 1,
    };
    if (event.key in moves) {
      event.preventDefault();
      selected = Math.max(0, Math.min(total - 1, moves[event.key]));
      reveal(selected);
    } else if (event.key === 'Enter' && selected >= 0) {
      event.preventDefault();
      void startStation(stations[selected]);
    } else if (isFavoriteKey(event) && selected >= 0) {
      event.preventDefault();
      void toggleStationFavorite(stations[selected]);
    }
  }

  /** `MP3 128`, `AAC 64`, `HLS`. */
  function quality(station: Station): string {
    const parts = [station.hls ? 'HLS' : station.codec?.toUpperCase(), station.bitrateKbps || null];
    return parts.filter(Boolean).join(' ');
  }

  /** Where the station is from, then up to three of its genres. */
  function detail(station: Station): string {
    const place = station.region || (station.countryCode ? countryName(station.countryCode) : '');
    return [place, ...station.tags.slice(0, 3)].filter(Boolean).join(' · ');
  }
</script>

{#if total > 0}
  <div
    class="list"
    role="listbox"
    tabindex="0"
    aria-keyshortcuts={FAVORITE_KEY}
    aria-label={t('radio.list.label')}
    aria-activedescendant={activeId}
    style={virtual ? `height:${total * ROW}px` : undefined}
    {onkeydown}
    {oncontextmenu}
    onclick={(event) => {
      const i = rowIndex(event);
      if (i !== null) selected = i;
    }}
    ondblclick={(event) => {
      const i = rowIndex(event);
      if (i !== null) void startStation(stations[i]);
    }}
    {@attach follow}
  >
    <div class="window" class:flow={!virtual} style={virtual ? `transform:translateY(${range.start * ROW}px)` : undefined}>
      {#each rows as { index, station } (index)}
        {@const playing = station.id === playingId}
        <div
          id={rowId(index)}
          class="row"
          class:selected={selected === index}
          class:playing
          role="option"
          aria-selected={selected === index}
          aria-setsize={total}
          aria-posinset={index + 1}
          data-index={index}
        >
          <span class="num">
            <RowPlay
              number={index + 1}
              name={station.name}
              current={playing}
              revealed={selected === index}
              onplay={() => void startStation(station)}
            />
          </span>
          <StationTile name={station.name} coverId={station.coverId} size={40} />
          <span class="main">
            <span class="name">{station.name}</span>
            <span class="detail">{detail(station)}</span>
          </span>
          <span class="dist">
            {#if showDistance && station.distanceKm != null}
              {t('radio.near.distance', { km: Math.max(1, Math.round(station.distanceKm)) })}
            {/if}
          </span>
          <span class="quality">{quality(station)}</span>
          <Heart
            kind="station"
            id={station.id}
            size={16}
            tabindex={-1}
            ghost
            revealed={selected === index}
            label={t('favorites.toggleItem', { name: station.name })}
            onflip={() => void toggleStationFavorite(station)}
          />
        </div>
      {/each}
    </div>
  </div>
{/if}

{#if menu}
  <StationMenu {...menu} onclose={() => (menu = null)} />
{/if}

<style>
  .list {
    position: relative;
    outline: none;
  }

  .window {
    position: absolute;
    inset: 0 0 auto 0;
    will-change: transform;
  }

  .window.flow {
    position: static;
    will-change: auto;
  }

  .row {
    height: 56px;
    display: grid;
    grid-template-columns: 40px 40px minmax(0, 1fr) 64px 76px 30px;
    column-gap: 14px;
    align-items: center;
    padding: 0 12px;
    border-radius: 8px;
    font-size: 13.5px;
    cursor: default;
    transition: background-color var(--dur) ease-out;
  }

  .row:hover {
    background: rgba(255, 255, 255, 0.04);
  }

  .row.selected {
    background: rgba(255, 255, 255, 0.075);
  }

  .row:hover :global(.heart.hidden) {
    opacity: 1;
    pointer-events: auto;
  }

  .list:focus-visible .row.selected {
    box-shadow: inset 0 0 0 1.5px rgba(77, 166, 255, 0.7);
  }

  .num {
    display: flex;
    justify-content: flex-end;
    text-align: right;
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
    color: rgba(243, 243, 245, 0.45);
  }

  .main {
    display: grid;
    gap: 2px;
    min-width: 0;
  }

  .name,
  .detail {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .name {
    font-weight: 500;
  }

  .playing .name {
    color: #fff;
    font-weight: 600;
  }

  .detail {
    font-size: 12px;
    color: var(--text-dim);
  }

  .dist,
  .quality {
    text-align: right;
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
    color: rgba(243, 243, 245, 0.45);
    white-space: nowrap;
  }
</style>
