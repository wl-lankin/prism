<script lang="ts">
  import { untrack } from 'svelte';
  import { formatDuration, isLive, liveLines, sourceQualityLabel, streamedFrom, ytmIdle, ytmLines } from '../format';
  import { t } from '../i18n';
  import { coverUrl, playerJump, playerQueueRange } from '../ipc';
  import { evictPages, pagesFor } from '../pages';
  import { FAVORITE_KEY, isFavoriteKey, toggleFavorite } from '../stores/favorites.svelte';
  import { go } from '../stores/nav.svelte';
  import { player } from '../stores/player.svelte';
  import { currentTitle, findStation, toggleStationFavorite } from '../stores/radio.svelte';
  import { settings } from '../stores/settings.svelte';
  import { ytm } from '../stores/ytm.svelte';
  import type { FlowVariant, QueueItem, Track } from '../types';
  import { visibleRange } from '../virtual';
  import Backdrop from './Backdrop.svelte';
  import Cover from './Cover.svelte';
  import Heart from './Heart.svelte';
  import Icon from './Icon.svelte';
  import LivePill from './LivePill.svelte';
  import NowPlayingBars from './NowPlayingBars.svelte';
  import RowPlay from './RowPlay.svelte';
  import StationTile from './StationTile.svelte';

  /** What plays now, as large as the window allows, beside the whole queue. The queue is
   *  virtualized: rows of 52 px, read in pages of 200 and dropped whenever its ids change. */
  const ROW = 52;
  const PAGE = 200;
  const MAX_PAGES = 10;
  /** After the user scrolled the queue, a track change leaves it where it is for this long. */
  const HOLD_MS = 5000;

  const track = $derived(player.track);
  /** YouTube Music answers the transport: this view shows its state, and the queue is its own. */
  const yt = $derived(player.active === 'ytm');
  const idle = $derived(yt ? ytmIdle(ytm.state) : player.status === 'idle' || !track);
  const total = $derived(player.queueLength);
  const current = $derived(player.queueIndex);
  /** Dropped files ("file:" ids) are not in the library: no album, no artist page, no heart. */
  const inLibrary = $derived(!!track && !track.id.startsWith('file:'));
  /** Source and how it plays; a Navidrome transcode also names the file behind it. */
  const qualityText = $derived.by(() => {
    if (!track) return '';
    const label = sourceQualityLabel(track, (s) => t(`source.${s}`), settings.navidromeQuality);
    const file = streamedFrom(track, settings.navidromeQuality);
    return file ? t('quality.from', { stream: label, file }) : label;
  });
  /** A station: no seek, no queue to speak of, the stream title over the station name. */
  const live = $derived(!idle && isLive(track));
  const lines = $derived(track ? liveLines(track, live ? currentTitle(track) : null) : null);
  // A station without a logo tints the view with a plain radio-blue tile.
  function blueTile(): string {
    const fill = getComputedStyle(document.documentElement).getPropertyValue('--src-radio').trim() || '#4da6ff';
    return `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="${fill}"/></svg>`)}`;
  }
  const backdrop = $derived(
    yt ? coverUrl(ytm.state.coverId, 96) : (coverUrl(track?.coverId ?? null, 96) ?? (live ? blueTile() : null)),
  );

  const flow = $derived((player.flow ?? null) as FlowVariant | null);
  const heading = $derived(flow ? t('nowPlaying.flow', { variant: variantName(flow) }) : t('nowPlaying.queue'));

  function variantName(variant: FlowVariant): string {
    if (variant.kind === 'genre') return variant.genre;
    return t(variant.kind === 'favorites' ? 'nowPlaying.flowFavorites' : 'nowPlaying.flowLibrary');
  }

  /** Each page remembers the queue version it was read at; others are ignored. */
  const pages = new Map<number, { version: number; items: QueueItem[] }>();
  const pending = new Set<string>();
  let loaded = $state(0);

  let scroller = $state<HTMLDivElement>();
  let scrollTop = $state(0);
  let viewport = $state(0);
  let selected = $state(-1);
  let userScrolledAt = 0;

  const range = $derived(visibleRange(scrollTop, viewport, ROW, total, 8));

  /** Requests the pages the visible rows need, and always the one holding the current entry. */
  function ensure() {
    const version = player.queueVersion;
    const needed = new Set(pagesFor(range.start, range.end, PAGE));
    if (total > 0) needed.add(Math.floor(Math.min(current, total - 1) / PAGE));
    for (const page of needed) {
      const key = `${version}:${page}`;
      if (pages.get(page)?.version === version || pending.has(key)) continue;
      pending.add(key);
      playerQueueRange(page * PAGE, PAGE)
        .then((items) => {
          if (version !== player.queueVersion) return;
          pages.set(page, { version, items: items ?? [] });
          const keep = [...pagesFor(range.start, range.end, PAGE), Math.floor(current / PAGE)];
          for (const old of evictPages(pages.keys(), keep, MAX_PAGES)) pages.delete(old);
          loaded++;
        })
        .catch((err) => console.error('loading the queue failed', err))
        .finally(() => pending.delete(key));
    }
  }

  $effect(() => {
    void player.queueVersion;
    void range.start;
    void range.end;
    void current;
    const frame = requestAnimationFrame(ensure);
    return () => cancelAnimationFrame(frame);
  });

  function entry(i: number): QueueItem | undefined {
    const page = pages.get(Math.floor(i / PAGE));
    if (page?.version !== player.queueVersion) return undefined;
    return page.items[i - Math.floor(i / PAGE) * PAGE];
  }

  const rows = $derived.by(() => {
    void loaded;
    void player.queueVersion;
    const out: { index: number; item: QueueItem | undefined }[] = [];
    for (let i = range.start; i < range.end; i++) out.push({ index: i, item: entry(i) });
    return out;
  });

  const activeId = $derived(
    selected >= range.start && selected < range.end ? `queue-${selected}` : undefined,
  );

  function reducedMotion(): boolean {
    return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;
  }

  /** Puts the current entry a third of the way down the queue. */
  function reveal(smooth: boolean) {
    if (!scroller || total === 0) return;
    const max = Math.max(0, total * ROW - viewport);
    const top = Math.round(Math.max(0, Math.min(max, current * ROW - viewport * 0.3)));
    if (Math.abs(top - scroller.scrollTop) < 1) return;
    if (smooth && !reducedMotion() && typeof scroller.scrollTo === 'function') {
      scroller.scrollTo({ top, behavior: 'smooth' });
    } else {
      scroller.scrollTop = top;
      scrollTop = top;
    }
  }

  // On a track change the current entry comes into view, unless the user just scrolled.
  let first = true;
  $effect(() => {
    void current;
    void track?.id;
    untrack(() => {
      if (first) {
        first = false;
        reveal(false);
      } else if (Date.now() - userScrolledAt >= HOLD_MS) {
        reveal(true);
      }
    });
  });

  // Once the queue has a height, the opening position is set again against it.
  let measured = false;
  $effect(() => {
    if (viewport > 0 && !measured) {
      measured = true;
      untrack(() => reveal(false));
    }
  });

  function touched() {
    userScrolledAt = Date.now();
  }

  /** Wheel, touch and a grab of the scrollbar count as the user scrolling; a scroll this view
   *  makes itself does not. */
  function userScrolls(node: HTMLElement) {
    const events = ['wheel', 'pointerdown', 'touchstart'] as const;
    for (const name of events) node.addEventListener(name, touched, { passive: true });
    return () => {
      for (const name of events) node.removeEventListener(name, touched);
    };
  }

  function onscroll() {
    if (scroller) scrollTop = scroller.scrollTop;
  }

  function jump(index: number) {
    playerJump(index).catch((err) => console.error('jumping in the queue failed', err));
  }

  function rowIndex(event: MouseEvent): number | null {
    if ((event.target as HTMLElement).closest('button')) return null;
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-index]');
    return row ? Number(row.dataset.index) : null;
  }

  function keepVisible(i: number) {
    if (!scroller) return;
    const top = i * ROW;
    if (top < scroller.scrollTop) scroller.scrollTop = top;
    else if (top + ROW > scroller.scrollTop + viewport) scroller.scrollTop = top + ROW - viewport;
  }

  function onkeydown(event: KeyboardEvent) {
    if (total === 0) return;
    touched();
    const step = Math.max(1, Math.floor(viewport / ROW) - 1);
    const from = selected >= 0 ? selected : current;
    const moves: Record<string, number> = {
      ArrowDown: from + 1,
      ArrowUp: from - 1,
      PageDown: from + step,
      PageUp: from - step,
      Home: 0,
      End: total - 1,
    };
    if (event.key in moves) {
      event.preventDefault();
      selected = Math.max(0, Math.min(total - 1, moves[event.key]));
      keepVisible(selected);
    } else if (event.key === 'Enter' && selected >= 0) {
      event.preventDefault();
      jump(selected);
    } else if (isFavoriteKey(event) && selected >= 0) {
      const item = entry(selected)?.track;
      if (!item || item.id.startsWith('file:')) return;
      event.preventDefault();
      if (isLive(item)) {
        const station = findStation(item.id);
        if (station) void toggleStationFavorite(station);
      } else void toggleFavorite('track', item.id);
    }
  }

  function openAlbum(t: Track) {
    if (t.albumId) go('album', t.albumId);
  }
</script>

<div class="np">
  {#if backdrop}
    <Backdrop src={backdrop} spread={1.15} />
  {/if}

  {#if idle || (!yt && !track)}
    <div class="empty">
      <span class="empty-art" aria-hidden="true"><Icon name="songs" size={40} /></span>
      <h1>{t('transport.idleTitle')}</h1>
      <p>{t('nowPlaying.emptyLead')}</p>
    </div>
  {:else if yt}
    {@const yl = ytmLines(ytm.state)}
    <div class="stage">
      <div class="art">
        <Cover coverId={ytm.state.coverId} fluid cache={600} radius="18px" alt={ytm.state.album} />
      </div>
    </div>

    <div class="side">
      <header class="info">
        <h1>{yl.title}</h1>
        <p class="byline">
          {#if yl.sub}<span class="artist">{yl.sub}</span>{/if}
          {#if ytm.state.album}<span class="album">{ytm.state.album}</span>{/if}
        </p>
        <div class="source">
          <span class="quality" style="--dot:var(--src-youtube)" title={t('ytm.noHeart')}>
            <span class="qtext">{t('ytm.source')}</span>
          </span>
        </div>
      </header>
      <p class="ytm-note">{t('ytm.queueNote')}</p>
    </div>
  {:else if track}
    <div class="stage">
      <div class="art">
        {#if live}
          <StationTile name={track.title} coverId={track.coverId} fluid cache={300} radius="18px" />
        {:else}
          <Cover coverId={track.coverId} fluid cache={600} radius="18px" alt={track.album} />
        {/if}
      </div>
    </div>

    <div class="side">
      <header class="info">
        <h1>
          {#if live && lines}
            {lines.title}
          {:else if inLibrary && track.albumId}
            <button type="button" class="link" onclick={() => openAlbum(track)}>{track.title}</button>
          {:else}
            {track.title}
          {/if}
        </h1>
        <p class="byline">
          {#if live && lines}
            <span class="artist">{lines.sub}</span>
          {:else if inLibrary && track.artist}
            <button type="button" class="link artist" onclick={() => go('artist', track.artist)}>{track.artist}</button>
          {:else if track.artist}
            <span class="artist">{track.artist}</span>
          {/if}
          {#if inLibrary && track.album && track.albumId}
            <button type="button" class="link album" onclick={() => openAlbum(track)}>{track.album}</button>
          {:else if track.album}
            <span class="album">{track.album}</span>
          {/if}
        </p>
        <div class="source">
          <span class="quality" style="--dot:var(--src-{track.source})">
            <span class="qtext" title={qualityText}>{qualityText}</span>
          </span>
          {#if live}
            <LivePill />
          {/if}
          {#if inLibrary}
            <Heart kind={live ? 'station' : 'track'} id={track.id} round label={t('favorites.toggleItem', { name: track.title })} />
          {/if}
        </div>
      </header>

      <section class="queue" aria-labelledby="queue-heading">
        <div class="qhead">
          <h2 id="queue-heading" class:flow={!!flow}>{heading}</h2>
          {#if total > 0}
            <span class="pos">{t('nowPlaying.position', { n: Math.min(current + 1, total), total })}</span>
          {/if}
        </div>

        <div
          class="scroll"
          bind:this={scroller}
          bind:clientHeight={viewport}
          {onscroll}
          {@attach userScrolls}
        >
          <div
            class="list"
            role="listbox"
            tabindex="0"
            aria-keyshortcuts={FAVORITE_KEY}
            aria-labelledby="queue-heading"
            aria-activedescendant={activeId}
            style="height:{total * ROW}px"
            {onkeydown}
            onclick={(event) => {
              const i = rowIndex(event);
              if (i === null) return;
              selected = i;
              jump(i);
            }}
          >
            <div class="window" style="transform:translateY({range.start * ROW}px)">
              {#each rows as row (row.index)}
                {@const item = row.item}
                {@const isCurrent = row.index === current}
                <div
                  id="queue-{row.index}"
                  class="qrow"
                  class:current={isCurrent}
                  class:past={row.index < current}
                  class:selected={selected === row.index}
                  role="option"
                  aria-selected={selected === row.index}
                  aria-current={isCurrent ? 'true' : undefined}
                  aria-setsize={total}
                  aria-posinset={row.index + 1}
                  data-index={row.index}
                >
                  <span class="num">
                    {#if item?.track}
                      <RowPlay
                        number={row.index + 1}
                        name={item.track.title}
                        current={isCurrent}
                        revealed={selected === row.index}
                        onplay={() => {
                          selected = row.index;
                          jump(row.index);
                        }}
                      />
                    {:else if isCurrent}
                      <NowPlayingBars playing={player.status === 'playing'} size={12} />
                    {:else}
                      {row.index + 1}
                    {/if}
                  </span>
                  {#if item?.track}
                    {@const q = item.track}
                    {#if isLive(q)}
                      <StationTile name={q.title} coverId={q.coverId} size={36} radius="6px" />
                    {:else}
                      <Cover coverId={q.coverId} size={36} cache={96} radius="6px" />
                    {/if}
                    <span class="name">
                      <span class="qtitle">{q.title}</span>
                      <span class="qartist">{q.artist}</span>
                    </span>
                    {#if q.id.startsWith('file:')}
                      <span></span>
                    {:else}
                      <Heart kind={isLive(q) ? 'station' : 'track'} id={q.id} size={16} tabindex={-1} ghost revealed={selected === row.index} label={t('favorites.toggleItem', { name: q.title })} />
                    {/if}
                    <span class="time">
                      {#if isLive(q)}<LivePill size="small" />{:else}{formatDuration(q.durationMs)}{/if}
                    </span>
                  {:else if item}
                    <span class="ghost-cover"></span>
                    <span class="name"><span class="qartist">{t('nowPlaying.missing')}</span></span>
                    <span></span>
                    <span></span>
                  {:else}
                    <span class="ghost-cover"></span>
                    <span class="name"><span class="ghost" style="width:{40 + ((row.index * 37) % 35)}%"></span></span>
                    <span></span>
                    <span></span>
                  {/if}
                </div>
              {/each}
            </div>
          </div>
        </div>
      </section>
    </div>
  {/if}
</div>

<style>
  /* A size container, so the cover can be as tall as the view allows (up to 600 px). */
  .np {
    position: relative;
    height: 100%;
    box-sizing: border-box;
    container-type: size;
    display: grid;
    grid-template-columns: auto minmax(320px, 1fr);
    column-gap: clamp(28px, 3.4vw, 52px);
    padding: calc(4px + var(--view-top, 0px)) 32px 28px;
    overflow: hidden;
  }

  .stage,
  .side {
    position: relative;
    min-height: 0;
  }

  .stage {
    display: flex;
    align-items: flex-start;
  }

  .art {
    width: min(600px, calc(100cqh - 32px), 52cqw);
    border-radius: 18px;
    box-shadow:
      0 40px 80px -28px rgba(0, 0, 0, 0.9),
      0 0 0 1px rgba(255, 255, 255, 0.07);
  }

  .side {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .info {
    padding: 6px 0 22px;
  }

  .ytm-note {
    margin: 0;
    max-width: 38ch;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-dim);
  }

  h1 {
    margin: 0;
    font-family: var(--font-display);
    font-size: clamp(28px, 3.1vw, 44px);
    font-weight: 600;
    letter-spacing: -0.025em;
    line-height: 1.06;
    text-wrap: balance;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .link {
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    letter-spacing: inherit;
    text-align: left;
    cursor: pointer;
    text-decoration: underline;
    text-decoration-color: transparent;
    text-underline-offset: 0.18em;
    text-decoration-thickness: 1px;
    transition: text-decoration-color var(--dur) ease-out;
  }

  .link:hover {
    text-decoration-color: rgba(243, 243, 245, 0.45);
  }

  .link:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 3px;
    border-radius: 4px;
  }

  .byline {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    margin: 12px 0 0;
    min-width: 0;
  }

  .artist {
    max-width: 100%;
    font-size: 17px;
    font-weight: 500;
    color: var(--text);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .album {
    max-width: 100%;
    font-size: 14px;
    color: var(--text-dim);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .source {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 18px;
  }

  .quality {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 11px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.055);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.07);
    color: var(--text-dim);
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.01em;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    min-width: 0;
  }

  /* A transcode label is longer; a narrow header cuts it, the tooltip has it whole. */
  .qtext {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .quality::before {
    content: '';
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--dot);
    box-shadow: 0 0 8px var(--dot);
  }

  .queue {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--line);
  }

  .qhead {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    padding: 16px 0 10px;
  }

  h2 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 16px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  /* In Flow the heading carries the prism's spectrum. */
  h2.flow {
    background: linear-gradient(90deg, var(--spec-1), var(--spec-2) 35%, var(--spec-3) 68%, var(--spec-4));
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }

  .pos {
    font-size: 12px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .scroll {
    scrollbar-color: auto;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    margin: 0 -12px;
    padding: 0 6px 0 0;
    mask-image: linear-gradient(to bottom, #000 calc(100% - 28px), transparent);
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

  .qrow {
    position: relative;
    display: grid;
    grid-template-columns: 28px 36px minmax(0, 1fr) 30px minmax(48px, auto);
    column-gap: 12px;
    align-items: center;
    height: 52px;
    padding: 0 12px;
    border-radius: 10px;
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      opacity var(--dur) ease-out;
  }

  .qrow:hover {
    background: rgba(255, 255, 255, 0.045);
  }

  .qrow.selected {
    background: rgba(255, 255, 255, 0.075);
  }

  /* What already played recedes; it stays reachable with a click. */
  .qrow.past {
    opacity: 0.48;
  }

  .qrow.past:hover,
  .qrow.past.selected {
    opacity: 0.85;
  }

  .qrow.current {
    background: rgba(255, 255, 255, 0.09);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  /* The current entry is marked by a thin spectrum edge, the prism's own light. */
  .qrow.current::before {
    content: '';
    position: absolute;
    left: 0;
    top: 9px;
    bottom: 9px;
    width: 3px;
    border-radius: 3px;
    background: linear-gradient(180deg, var(--spec-1), var(--spec-2), var(--spec-3), var(--spec-4));
  }

  .qrow:hover :global(.heart.hidden) {
    opacity: 1;
    pointer-events: auto;
  }

  .list:focus-visible .qrow.selected {
    box-shadow: inset 0 0 0 1.5px rgba(77, 166, 255, 0.7);
  }

  .num,
  .time {
    display: flex;
    justify-content: flex-end;
    font-size: 12px;
    color: rgba(243, 243, 245, 0.45);
    font-variant-numeric: tabular-nums;
  }

  .name {
    display: grid;
    gap: 2px;
    min-width: 0;
  }

  .qtitle,
  .qartist {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .qtitle {
    font-size: 13.5px;
    font-weight: 500;
  }

  .qartist {
    font-size: 12.5px;
    color: var(--text-dim);
  }

  .current .qtitle {
    color: #fff;
    font-weight: 600;
  }

  .ghost-cover,
  .ghost {
    display: block;
    height: 10px;
    border-radius: 5px;
    background: rgba(255, 255, 255, 0.05);
  }

  .ghost-cover {
    width: 36px;
    height: 36px;
    border-radius: 6px;
  }

  .empty {
    position: relative;
    grid-column: 1 / -1;
    align-self: center;
    justify-self: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    max-width: 380px;
    text-align: center;
  }

  .empty-art {
    display: grid;
    place-items: center;
    width: 92px;
    height: 92px;
    margin-bottom: 8px;
    border-radius: 22px;
    background: linear-gradient(145deg, #25252c, #18181d);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
    color: rgba(243, 243, 245, 0.35);
  }

  .empty h1 {
    font-size: 26px;
  }

  .empty p {
    margin: 0;
    font-size: 14px;
    line-height: 1.55;
    color: var(--text-dim);
  }

  /* Narrow windows: the cover gives way to the queue. */
  @container (max-width: 760px) {
    .art {
      width: min(600px, calc(100cqh - 32px), 40cqw);
    }
  }
</style>
