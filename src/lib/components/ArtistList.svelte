<script lang="ts">
  import type { Snippet } from 'svelte';
  import { t, tc } from '../i18n';
  import { saveScroll, savedScroll } from '../scroll';
  import { go, nav } from '../stores/nav.svelte';
  import { player } from '../stores/player.svelte';
  import type { Artist } from '../types';
  import { visibleRange } from '../virtual';
  import Cover from './Cover.svelte';
  import NowPlayingBars from './NowPlayingBars.svelte';
  import SourceBadge from './SourceBadge.svelte';
  import ViewHead from './ViewHead.svelte';

  /** Every album artist, virtualized; a click opens the artist page, which is one page per
   *  name over every source. */
  let {
    artists,
    showSource = false,
    header,
  }: {
    artists: Artist[];
    /** Marks each artist with its source; for lists that mix sources. */
    showSource?: boolean;
    /** Replaces the default heading. */
    header?: Snippet;
  } = $props();

  /** Names listed more than once (one artist per source): those rows name their source, so
   *  the two read apart. One pass per list, not per row. */
  const shared = $derived.by(() => {
    const out = new Set<string>();
    if (!showSource) return out;
    const seen = new Set<string>();
    for (const artist of artists) {
      if (seen.has(artist.name)) out.add(artist.name);
      else seen.add(artist.name);
    }
    return out;
  });

  const ROW = 64;
  /** Each source view keeps its own scroll position. */
  const KEY = $derived(`artists:${nav.source ?? 'all'}`);

  let scroller = $state<HTMLDivElement>();
  let listTop = $state(0);
  let scrollTop = $state(0);
  let viewport = $state(0);

  const range = $derived(visibleRange(scrollTop - listTop, viewport, ROW, artists.length, 6));
  const visible = $derived(artists.slice(range.start, range.end));
  const playingArtist = $derived(player.track?.albumArtist ?? null);

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

  function counts(artist: Artist): string {
    const text = `${tc('view.count.albums', artist.albumCount)}, ${tc('view.count.songs', artist.trackCount)}`;
    return shared.has(artist.name) ? `${t(`source.${artist.source}`)} · ${text}` : text;
  }
</script>

<div class="scroll" bind:this={scroller} bind:clientHeight={viewport} {onscroll}>
  {#if header}
    {@render header()}
  {:else}
    <ViewHead title={t('nav.artists')} meta={tc('view.count.artists', artists.length)} />
  {/if}

  <ul
    class="list"
    style="height:{artists.length * ROW}px"
    {@attach (node) => {
      listTop = node.offsetTop;
    }}
  >
    {#each visible as artist, i (`${artist.source}:${artist.id}`)}
      <li style="transform:translateY({(range.start + i) * ROW}px)">
        <button type="button" class="row" onclick={() => go('artist', artist.name)}>
          <span class="avatar">
            <Cover coverId={artist.coverId} size={44} cache={96} radius="50%" />
            {#if showSource && artist.source !== 'local'}<SourceBadge source={artist.source} size={9} />{/if}
          </span>
          <span class="text">
            <span class="name">{artist.name}</span>
            <span class="counts">{counts(artist)}</span>
          </span>
          {#if playingArtist === artist.name}
            <NowPlayingBars playing={player.status === 'playing'} size={12} />
          {/if}
        </button>
      </li>
    {/each}
  </ul>
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

  .list {
    position: relative;
    list-style: none;
    margin: 0 -12px;
    padding: 0;
  }

  li {
    position: absolute;
    inset: 0 0 auto 0;
    height: 64px;
  }

  .row {
    width: 100%;
    height: 60px;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 0 12px;
    border: 0;
    border-radius: 12px;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
    transition: background-color var(--dur) ease-out;
  }

  .row:hover {
    background: rgba(255, 255, 255, 0.045);
  }

  .row:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: -2px;
  }

  .avatar {
    position: relative;
    flex: none;
    display: block;
    width: 44px;
    height: 44px;
  }

  /* On the portrait's edge, like a presence dot. */
  .avatar :global(.source-badge) {
    position: absolute;
    right: 0;
    bottom: 1px;
  }

  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .name,
  .counts {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .name {
    font-size: 14.5px;
    font-weight: 560;
  }

  .counts {
    font-size: 12.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
</style>
