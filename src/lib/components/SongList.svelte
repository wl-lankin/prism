<script lang="ts">
  import type { Snippet } from 'svelte';
  import { formatDuration } from '../format';
  import { t, tc } from '../i18n';
  import { libraryTracksRange } from '../ipc';
  import { playSongsFrom } from '../play';
  import { saveScroll, savedScroll } from '../scroll';
  import { countOf, library } from '../stores/library.svelte';
  import { go, nav } from '../stores/nav.svelte';
  import { FAVORITE_KEY, isFavoriteKey, toggleFavorite } from '../stores/favorites.svelte';
  import { player } from '../stores/player.svelte';
  import type { Source, Track } from '../types';
  import { evictPages, pagesFor } from '../pages';
  import { visibleRange } from '../virtual';
  import { menuPoint } from '../menu';
  import Cover from './Cover.svelte';
  import TrackMenu, { type TrackMenuState } from './TrackMenu.svelte';
  import Heart from './Heart.svelte';
  import RowPlay from './RowPlay.svelte';
  import SourceBadge from './SourceBadge.svelte';
  import ViewHead from './ViewHead.svelte';

  /** Every track (of one source, or of every source) in one virtualized list. Rows are
   *  fetched in pages of 200 when they come into view and kept in a cache that is dropped
   *  whenever the library changes. */
  let {
    source,
    showSource = false,
    header,
  }: {
    /** Lists only this source's tracks; none lists every source. */
    source?: Source;
    /** Marks each row's cover with its source; for lists that mix sources. */
    showSource?: boolean;
    /** Replaces the default heading. */
    header?: Snippet;
  } = $props();

  const ROW = 44;
  const PAGE = 200;
  /** Each source view keeps its own scroll position. */
  const KEY = $derived(`songs:${nav.source ?? 'all'}`);

  const total = $derived((source ? (library.trackCounts ? countOf(library.trackCounts, source) : undefined) : library.trackCounts?.all) ?? 0);

  /** At most this many pages stay cached; the ones farthest from the view are dropped. */
  const MAX_PAGES = 20;

  /** Each page remembers the library version it was read at; others are ignored. */
  const pages = new Map<number, { version: number; tracks: Track[] }>();
  /** `${version}:${page}` of requests in flight. */
  const pending = new Set<string>();
  /** Bumped when a page arrives, so the rows re-read the cache. */
  let loaded = $state(0);
  /** Bumped a moment after a page failed: the pages in view are asked for again, so a failed
   *  page comes back without the list having to scroll first. A page is tried at most
   *  `MAX_ATTEMPTS` times; when the range changes, its count starts over. */
  let retry = $state(0);
  const RETRY_MS = 1500;
  const MAX_ATTEMPTS = 3;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => () => clearTimeout(retryTimer));
  /** Failed attempts per `${version}:${page}`, for the range they were made in. */
  const failures = new Map<string, number>();
  let failuresRange = '';

  let menu = $state<TrackMenuState | null>(null);

  let scroller = $state<HTMLDivElement>();
  let listTop = $state(0);
  let scrollTop = $state(0);
  let viewport = $state(0);
  let selected = $state(-1);

  const range = $derived(visibleRange(scrollTop - listTop, viewport, ROW, total, 8));
  const playingId = $derived(player.track?.id ?? null);

  /** Requests the pages the current range needs. Runs one frame after the range settles,
   *  so a fast scroll does not ask for every page it crosses. */
  function ensure() {
    const version = library.version;
    const now = `${version}:${range.start}:${range.end}`;
    if (now !== failuresRange) {
      failuresRange = now;
      failures.clear();
    }
    const needed = pagesFor(range.start, range.end, PAGE);
    for (const page of needed) {
      const key = `${version}:${page}`;
      if (pages.get(page)?.version === version || pending.has(key)) continue;
      if ((failures.get(key) ?? 0) >= MAX_ATTEMPTS) continue;
      pending.add(key);
      libraryTracksRange(page * PAGE, PAGE, source)
        .then((tracks) => {
          if (version !== library.version) return;
          pages.set(page, { version, tracks: tracks ?? [] });
          const visible = pagesFor(range.start, range.end, PAGE);
          for (const old of evictPages(pages.keys(), visible, MAX_PAGES)) pages.delete(old);
          loaded++;
        })
        .catch((err) => {
          console.error('loading songs failed', err);
          const attempts = (failures.get(key) ?? 0) + 1;
          failures.set(key, attempts);
          if (attempts < MAX_ATTEMPTS) {
            clearTimeout(retryTimer);
            retryTimer = setTimeout(() => retry++, RETRY_MS);
          }
        })
        .finally(() => pending.delete(key));
    }
  }

  $effect(() => {
    void retry;
    void library.version;
    void range.start;
    void range.end;
    const frame = requestAnimationFrame(ensure);
    return () => cancelAnimationFrame(frame);
  });

  const rows = $derived.by(() => {
    void loaded;
    const version = library.version;
    const out: { index: number; track: Track | undefined }[] = [];
    for (let i = range.start; i < range.end; i++) {
      const page = pages.get(Math.floor(i / PAGE));
      out.push({ index: i, track: page?.version === version ? page.tracks[i % PAGE] : undefined });
    }
    return out;
  });

  /** The active row, only while it is rendered (aria-activedescendant must exist). */
  const activeId = $derived(
    selected >= range.start && selected < range.end ? `song-${selected}` : undefined,
  );

  $effect(() => {
    if (!scroller || viewport === 0) return;
    const top = savedScroll(KEY);
    if (top) scroller.scrollTop = top;
    scrollTop = scroller.scrollTop;
  });

  function onscroll() {
    if (!scroller) return;
    scrollTop = scroller.scrollTop;
    saveScroll(KEY, scrollTop);
  }

  /** Keeps row `i` inside the viewport (below the sticky column header). */
  function reveal(i: number) {
    if (!scroller) return;
    const top = listTop + i * ROW;
    const header = 36;
    if (top - header < scroller.scrollTop) scroller.scrollTop = top - header;
    else if (top + ROW > scroller.scrollTop + viewport) scroller.scrollTop = top + ROW - viewport;
  }

  /** The row a click landed on (rows carry `data-index`). */
  function rowIndex(event: MouseEvent): number | null {
    if ((event.target as HTMLElement).closest('button')) return null;
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-index]');
    return row ? Number(row.dataset.index) : null;
  }

  function trackAt(i: number): Track | undefined {
    const page = pages.get(Math.floor(i / PAGE));
    return page?.version === library.version ? page.tracks[i % PAGE] : undefined;
  }

  /** Right-click (or the context menu key) on a row: select it and open the track menu. */
  function oncontextmenu(event: MouseEvent) {
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-index]');
    const i = row ? Number(row.dataset.index) : selected;
    const track = i >= 0 ? trackAt(i) : undefined;
    if (!track) return;
    event.preventDefault();
    selected = i;
    const at = menuPoint(event, document.getElementById(`song-${i}`));
    menu = { trackIds: [track.id], ...at, albumId: track.albumId, artist: track.albumArtist || track.artist };
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
      void playSongsFrom(selected, source);
    } else if (isFavoriteKey(event) && selected >= 0) {
      const track = trackAt(selected);
      if (!track) return;
      event.preventDefault();
      void toggleFavorite('track', track.id);
    }
  }
</script>

<div class="scroll" bind:this={scroller} bind:clientHeight={viewport} {onscroll}>
  {#if header}
    {@render header()}
  {:else}
    <ViewHead title={t('nav.songs')} meta={tc('view.count.songs', total)} />
  {/if}

  <div class="cols" role="presentation">
    <span class="num">#</span>
    <span></span>
    <span>{t('songs.title')}</span>
    <span>{t('songs.artist')}</span>
    <span>{t('songs.album')}</span>
    <span></span>
    <span class="time">{t('songs.time')}</span>
  </div>

  <div
    class="list"
    role="listbox"
    tabindex="0"
    aria-keyshortcuts={FAVORITE_KEY}
    aria-label={t('nav.songs')}
    aria-activedescendant={activeId}
    style="height:{total * ROW}px"
    {onkeydown}
    {oncontextmenu}
    onclick={(event) => {
      const i = rowIndex(event);
      if (i !== null) selected = i;
    }}
    ondblclick={(event) => {
      const i = rowIndex(event);
      if (i !== null) void playSongsFrom(i, source);
    }}
    {@attach (node) => {
      listTop = node.offsetTop;
    }}
  >
    <div class="window" style="transform:translateY({range.start * ROW}px)">
      {#each rows as row (row.index)}
        {@const track = row.track}
        {@const playing = !!track && track.id === playingId}
        <div
          id="song-{row.index}"
          class="row"
          class:selected={selected === row.index}
          class:playing
          role="option"
          aria-selected={selected === row.index}
          aria-setsize={total}
          aria-posinset={row.index + 1}
          data-index={row.index}
        >
          <span class="num">
            {#if track}
              <RowPlay
                number={row.index + 1}
                name={track.title}
                current={playing}
                revealed={selected === row.index}
                onplay={() => void playSongsFrom(row.index, source)}
              />
            {:else}
              {row.index + 1}
            {/if}
          </span>
          {#if track}
            <span class="art">
              <Cover coverId={track.coverId} size={32} cache={96} radius="6px" />
              {#if showSource && track.source !== 'local'}<SourceBadge source={track.source} size={8} />{/if}
            </span>
            <span class="cell title">{track.title}</span>
            <span class="cell dim">{track.artist}</span>
            <button type="button" class="cell dim link" tabindex="-1" onclick={() => go('album', track.albumId, nav.source)}>
              {track.album}
            </button>
            <Heart kind="track" id={track.id} size={16} tabindex={-1} ghost revealed={selected === row.index} label={t('favorites.toggleItem', { name: track.title })} />
            <span class="time">{formatDuration(track.durationMs)}</span>
          {:else}
            <span class="ghost-cover"></span>
            <span class="ghost" style="width:46%"></span>
            <span class="ghost" style="width:38%"></span>
            <span class="ghost" style="width:52%"></span>
            <span></span>
            <span></span>
          {/if}
        </div>
      {/each}
    </div>
  </div>
</div>

{#if menu}
  <TrackMenu {...menu} onclose={() => (menu = null)} />
{/if}

<style>
  .scroll {
    scrollbar-color: auto;
    position: relative;
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 4px 32px 40px;
    box-sizing: border-box;
  }

  .cols,
  .row {
    display: grid;
    grid-template-columns: 40px 32px minmax(0, 2.2fr) minmax(0, 1.4fr) minmax(0, 1.4fr) 30px 56px;
    column-gap: 14px;
    align-items: center;
    padding: 0 12px;
  }

  .cols {
    position: sticky;
    top: -4px;
    z-index: 1;
    height: 36px;
    /* Full width of the column, frosted, so rows slide under it instead of being cut off. */
    margin: 0 -32px;
    padding: 0 44px;
    font-size: 12px;
    color: rgba(243, 243, 245, 0.45);
    background: rgba(20, 20, 24, 0.95);
    backdrop-filter: blur(18px) saturate(1.2);
    box-shadow: 0 1px 0 var(--line);
  }

  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
    display: flex;
    justify-content: flex-end;
  }

  .time {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .list {
    position: relative;
    outline: none;
  }

  .window {
    position: absolute;
    inset: 0 0 auto 0;
    will-change: transform;
  }

  .row {
    height: 44px;
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

  /* A song's heart shows on hover or selection (Heart's revealed), and always once it is set. */
  .row:hover :global(.heart.hidden) {
    opacity: 1;
    pointer-events: auto;
  }

  .list:focus-visible .row.selected {
    box-shadow: inset 0 0 0 1.5px rgba(77, 166, 255, 0.7);
  }

  .row .num,
  .row .time {
    font-size: 12.5px;
    color: rgba(243, 243, 245, 0.45);
  }

  .cell {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    min-width: 0;
  }

  .title {
    font-weight: 500;
  }

  .playing .title {
    color: #fff;
    font-weight: 600;
  }

  .dim {
    color: var(--text-dim);
  }

  .link {
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    text-align: left;
    cursor: pointer;
    justify-self: start;
    max-width: 100%;
  }

  .link:hover {
    color: var(--text);
    text-decoration: underline;
    text-underline-offset: 3px;
    text-decoration-color: rgba(243, 243, 245, 0.35);
  }

  .art {
    position: relative;
    display: block;
    width: 32px;
    height: 32px;
  }

  .art :global(.source-badge) {
    position: absolute;
    right: -2px;
    bottom: -2px;
  }

  .ghost-cover,
  .ghost {
    display: block;
    height: 10px;
    border-radius: 5px;
    background: rgba(255, 255, 255, 0.05);
  }

  .ghost-cover {
    width: 32px;
    height: 32px;
    border-radius: 6px;
  }
</style>
