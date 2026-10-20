<script lang="ts">
  import { formatDuration } from '../format';
  import { t, tc } from '../i18n';
  import { libraryTracksRange } from '../ipc';
  import { playSongsFrom } from '../play';
  import { saveScroll, savedScroll } from '../scroll';
  import { library } from '../stores/library.svelte';
  import { go } from '../stores/nav.svelte';
  import { player } from '../stores/player.svelte';
  import type { Track } from '../types';
  import { visibleRange } from '../virtual';
  import Cover from './Cover.svelte';
  import NowPlayingBars from './NowPlayingBars.svelte';
  import ViewHead from './ViewHead.svelte';

  /** Every track in one virtualized list. Rows are fetched in pages of 200 when they come
   *  into view and kept in a cache that is dropped whenever the library changes. */
  const ROW = 44;
  const PAGE = 200;
  const KEY = 'songs';

  const total = $derived(library.trackCount ?? 0);

  let pages = new Map<number, Track[]>();
  let pending = new Set<number>();
  /** Bumped when a page arrives, so the rows re-read the cache. */
  let loaded = $state(0);
  let cacheVersion = -1;

  let scroller = $state<HTMLDivElement>();
  let listTop = $state(0);
  let scrollTop = $state(0);
  let viewport = $state(0);
  let selected = $state(-1);

  const range = $derived(visibleRange(scrollTop - listTop, viewport, ROW, total, 8));
  const playingId = $derived(player.track?.id ?? null);

  function ensure(version: number, start: number, end: number) {
    if (version !== cacheVersion) {
      cacheVersion = version;
      pages = new Map();
      pending = new Set();
    }
    if (end <= start) return;
    for (let page = Math.floor(start / PAGE); page <= Math.floor((end - 1) / PAGE); page++) {
      if (pages.has(page) || pending.has(page)) continue;
      pending.add(page);
      libraryTracksRange(page * PAGE, PAGE)
        .then((tracks) => {
          if (version !== cacheVersion) return;
          pages.set(page, tracks ?? []);
          loaded++;
        })
        .catch((err) => console.error('loading songs failed', err))
        .finally(() => {
          if (version === cacheVersion) pending.delete(page);
        });
    }
  }

  $effect(() => ensure(library.version, range.start, range.end));

  const rows = $derived.by(() => {
    void loaded;
    void library.version;
    const out: { index: number; track: Track | undefined }[] = [];
    for (let i = range.start; i < range.end; i++) {
      out.push({ index: i, track: pages.get(Math.floor(i / PAGE))?.[i % PAGE] });
    }
    return out;
  });

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
      void playSongsFrom(selected);
    }
  }
</script>

<div class="scroll" bind:this={scroller} bind:clientHeight={viewport} {onscroll}>
  <ViewHead title={t('nav.songs')} meta={tc('view.count.songs', total)} />

  <div class="cols" role="presentation">
    <span class="num">#</span>
    <span></span>
    <span>{t('songs.title')}</span>
    <span>{t('songs.artist')}</span>
    <span>{t('songs.album')}</span>
    <span class="time">{t('songs.time')}</span>
  </div>

  <div
    class="list"
    role="listbox"
    tabindex="0"
    aria-label={t('nav.songs')}
    aria-activedescendant={selected >= 0 ? `song-${selected}` : undefined}
    style="height:{total * ROW}px"
    {onkeydown}
    onclick={(event) => {
      const i = rowIndex(event);
      if (i !== null) selected = i;
    }}
    ondblclick={(event) => {
      const i = rowIndex(event);
      if (i !== null) void playSongsFrom(i);
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
          data-index={row.index}
        >
          <span class="num">
            {#if playing}
              <NowPlayingBars playing={player.status === 'playing'} size={12} />
            {:else}
              {row.index + 1}
            {/if}
          </span>
          {#if track}
            <Cover coverId={track.coverId} size={32} cache={96} radius="6px" />
            <span class="cell title">{track.title}</span>
            <span class="cell dim">{track.artist}</span>
            <button type="button" class="cell dim link" tabindex="-1" onclick={() => go('album', track.albumId)}>
              {track.album}
            </button>
            <span class="time">{formatDuration(track.durationMs)}</span>
          {:else}
            <span class="ghost-cover"></span>
            <span class="ghost" style="width:46%"></span>
            <span class="ghost" style="width:38%"></span>
            <span class="ghost" style="width:52%"></span>
            <span></span>
          {/if}
        </div>
      {/each}
    </div>
  </div>
</div>

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
    grid-template-columns: 40px 32px minmax(0, 2.2fr) minmax(0, 1.4fr) minmax(0, 1.4fr) 56px;
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
    background: rgba(20, 20, 24, 0.84);
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
