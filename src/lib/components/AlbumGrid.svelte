<script lang="ts">
  import { untrack, type Snippet } from 'svelte';
  import { t } from '../i18n';
  import { libraryAlbumTracks } from '../ipc';
  import { playAlbum } from '../play';
  import { notify } from '../stores/notice.svelte';
  import { saveScroll, savedScroll } from '../scroll';
  import { go } from '../stores/nav.svelte';
  import { player } from '../stores/player.svelte';
  import type { Album } from '../types';
  import { anchorOffset, gridLayout, visibleRange } from '../virtual';
  import Cover from './Cover.svelte';
  import Icon from './Icon.svelte';
  import NowPlayingBars from './NowPlayingBars.svelte';
  import TrackMenu, { type TrackMenuState } from './TrackMenu.svelte';

  /** A card grid of albums, virtualized by rows: only the rows near the viewport exist. */
  let {
    albums,
    scrollKey,
    header,
  }: {
    albums: Album[];
    /** Where the scroll position is remembered. */
    scrollKey: string;
    /** Rendered above the grid, scrolling with it. */
    header?: Snippet;
  } = $props();

  const MIN_CARD = 180;
  const GAP = 24;
  /** Title and artist under the cover. */
  const CAPTION = 50;
  const ROW_GAP = 22;

  let scroller = $state<HTMLDivElement>();
  let scrollTop = $state(0);
  let viewport = $state(0);
  let width = $state(0);
  let headHeight = $state(0);
  /** The scroller's top padding. */
  const PAD_TOP = 4;
  const gridTop = $derived(PAD_TOP + headHeight);

  const layout = $derived(gridLayout(width, MIN_CARD, GAP));
  const rowHeight = $derived(layout.cardWidth + CAPTION + ROW_GAP);
  const rowCount = $derived(Math.ceil(albums.length / layout.columns));
  const range = $derived(visibleRange(scrollTop - gridTop, viewport, rowHeight, rowCount, 2));
  const rows = $derived.by(() => {
    const out: { index: number; albums: Album[] }[] = [];
    for (let r = range.start; r < range.end; r++) {
      out.push({ index: r, albums: albums.slice(r * layout.columns, (r + 1) * layout.columns) });
    }
    return out;
  });

  const playingAlbum = $derived(player.track?.albumId ?? null);

  let restored = false;
  let previous: { columns: number; rowHeight: number } | null = null;

  $effect(() => {
    if (!scroller || width === 0) return;
    const now = { columns: layout.columns, rowHeight };
    untrack(() => {
      if (!scroller) return;
      if (!restored) {
        // Restore once the grid has its size.
        restored = true;
        const top = savedScroll(scrollKey);
        if (top) scroller.scrollTop = top;
      } else if (previous && previous.columns !== now.columns) {
        // Keep the first visible album first when the window is resized.
        // The last scroll position seen before the resize: the browser may already have
        // clamped scrollTop to the new (shorter) grid.
        const offset = scrollTop - gridTop;
        if (offset > 0) {
          scroller.scrollTop =
            gridTop + anchorOffset(offset, previous.rowHeight, previous.columns, now.rowHeight, now.columns);
        }
      }
      previous = now;
      scrollTop = scroller.scrollTop;
    });
  });

  function onscroll() {
    if (!scroller) return;
    scrollTop = scroller.scrollTop;
    saveScroll(scrollKey, scrollTop);
  }

  let menu = $state<TrackMenuState | null>(null);
  /** Counts menu requests, so a slow track lookup never opens a menu a later one replaced. */
  let menuSeq = 0;

  /** Right-click (or the context menu key) on a card: "Add album to playlist" and the artist. */
  async function openMenu(event: MouseEvent, album: Album) {
    event.preventDefault();
    const seq = ++menuSeq;
    let x = event.clientX;
    let y = event.clientY;
    if (x === 0 && y === 0) {
      const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
      x = box.left + 24;
      y = box.top + 24;
    }
    try {
      const trackIds = (await libraryAlbumTracks(album.id)).map((track) => track.id);
      if (seq !== menuSeq) return;
      menu = { trackIds, x, y, album: true, artist: album.artist || undefined };
    } catch (err) {
      notify(t('flow.failed', { message: String(err) }));
    }
  }
</script>

<div class="scroll" bind:this={scroller} bind:clientHeight={viewport} {onscroll}>
  {#if header}
    <div class="head" bind:offsetHeight={headHeight}>{@render header()}</div>
  {/if}
  <div
    class="grid"
    bind:clientWidth={width}
    style="height:{rowCount * rowHeight - (rowCount ? ROW_GAP : 0)}px"
  >
    {#each rows as row (row.index)}
      <div
        class="row"
        style="transform:translateY({row.index * rowHeight}px); grid-template-columns:repeat({layout.columns}, {layout.cardWidth}px)"
      >
        {#each row.albums as album (album.id)}
          {@const playing = playingAlbum === album.id}
          <div class="card" role="none" oncontextmenu={(event) => openMenu(event, album)}>
            <button type="button" class="open" onclick={() => go('album', album.id)}>
              <span class="art">
                <Cover coverId={album.coverId} size={Math.round(layout.cardWidth)} fluid cache={300} radius="10px" />
              </span>
              <span class="title">
                {#if playing}<NowPlayingBars playing={player.status === 'playing'} size={11} />{/if}
                <span class="text">{album.title}</span>
              </span>
              <span class="artist">{album.artist}{album.year ? `, ${album.year}` : ''}</span>
            </button>
            <button
              type="button"
              class="play"
              aria-label={t('album.playTitle', { title: album.title })}
              title={t('album.playTitle', { title: album.title })}
              onclick={() => playAlbum(album.id)}
              style="top:{layout.cardWidth - 52}px"
            >
              <Icon name="play" size={18} filled />
            </button>
          </div>
        {/each}
      </div>
    {/each}
  </div>
</div>

{#if menu}
  <TrackMenu {...menu} onclose={() => (menu = null)} />
{/if}

<style>
  .scroll {
    scrollbar-color: auto;
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 4px 32px 40px;
    box-sizing: border-box;
  }

  .grid {
    position: relative;
  }

  .row {
    position: absolute;
    inset: 0 0 auto 0;
    display: grid;
    column-gap: 24px;
    will-change: transform;
  }

  .card {
    position: relative;
    min-width: 0;
  }

  .open {
    display: flex;
    flex-direction: column;
    width: 100%;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
    border-radius: 12px;
  }

  .open:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 4px;
  }

  .art {
    display: block;
    width: 100%;
    border-radius: 10px;
    box-shadow: 0 10px 24px -14px rgba(0, 0, 0, 0.9);
    transition: filter var(--dur) ease-out;
  }

  .card:hover .art {
    filter: brightness(1.08);
  }

  .title {
    display: flex;
    align-items: center;
    gap: 7px;
    margin-top: 11px;
    font-size: 13.5px;
    font-weight: 560;
    line-height: 18px;
    min-width: 0;
  }

  .text,
  .artist {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .artist {
    margin-top: 2px;
    font-size: 12.5px;
    line-height: 17px;
    color: var(--text-dim);
  }

  .play {
    position: absolute;
    right: 10px;
    width: 42px;
    height: 42px;
    display: grid;
    place-items: center;
    padding: 0 0 0 2px;
    border: 0;
    border-radius: 50%;
    background: var(--text);
    color: var(--bg);
    box-shadow: 0 8px 20px -6px rgba(0, 0, 0, 0.8);
    cursor: pointer;
    opacity: 0;
    transform: translateY(4px);
    transition:
      opacity var(--dur) ease-out,
      transform var(--dur) ease-out,
      background-color var(--dur) ease-out;
  }

  .card:hover .play,
  .play:focus-visible {
    opacity: 1;
    transform: none;
  }

  .play:hover {
    background: #fff;
  }

  .play:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
  }
</style>
