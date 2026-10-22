<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { formatDuration, isLive } from '../format';
  import { t, tc } from '../i18n';
  import { playerJump, playerMove, playerQueueRange, playerRemove } from '../ipc';
  import { evictPages, pagesFor } from '../pages';
  import { dragTarget, firstRow, QUEUE_TOGGLE_ID, stepAside, type QueueDrag } from '../queue';
  import { FAVORITE_KEY, isFavoriteKey, toggleFavorite } from '../stores/favorites.svelte';
  import { closeOverlay, openOverlay } from '../stores/overlays.svelte';
  import { player } from '../stores/player.svelte';
  import { findStation, toggleStationFavorite } from '../stores/radio.svelte';
  import { queuePanel, toggleQueuePanel } from '../stores/queue.svelte';
  import { ring } from '../stores/ring.svelte';
  import type { FlowVariant, QueueItem } from '../types';
  import { visibleRange } from '../virtual';
  import Cover from './Cover.svelte';
  import Heart from './Heart.svelte';
  import EmptyArt from './EmptyArt.svelte';
  import Icon from './Icon.svelte';
  import LivePill from './LivePill.svelte';
  import RowPlay from './RowPlay.svelte';
  import StationTile from './StationTile.svelte';

  /** "Up next": a sheet that slides in from the right over the content. The list starts at
   *  the track that plays (lit with its cover's colors), the played ones fold away above it.
   *  Upcoming rows are dragged to a new place (or moved with Alt+Up and Alt+Down) and removed
   *  with their X or Delete. Rows of 56 px are read in pages of 200 (`player_queue_range`)
   *  and read again whenever the queue's version changes; what is cached keeps showing
   *  until then, so a move or a removal made here does not flash. */
  const ROW = 56;
  const PAGE = 200;
  const MAX_PAGES = 6;
  const DRAG_START = 4;
  const SCROLL_EDGE = 40;

  const open = $derived(queuePanel.open);
  /** YouTube Music keeps its own queue; this panel only says so. */
  const yt = $derived(player.active === 'ytm');

  // The sheet lies over the content, above the YouTube Music view while it is open.
  $effect(() => {
    if (!open) return;
    untrack(() => openOverlay('queue'));
    return () => untrack(() => closeOverlay('queue'));
  });
  const total = $derived(player.queueLength);
  const current = $derived(player.queueIndex);
  const idle = $derived(player.status === 'idle' || !player.track);
  const empty = $derived(idle || total === 0);
  /** A station is a queue of one: no "up next" count and no tail note. */
  const live = $derived(!idle && isLive(player.track));
  const flow = $derived(yt ? null : ((player.flow ?? null) as FlowVariant | null));
  const upcoming = $derived(Math.max(0, total - current - 1));

  let showPlayed = $state(false);
  const start = $derived(firstRow(current, total, showPlayed));
  const count = $derived(Math.max(0, total - start));

  /** Under the last entry: what comes after it. */
  const tailNote = $derived(
    player.repeat === 'one' ? t('queue.repeatsOne') : player.repeat === 'all' ? t('queue.wraps') : t('queue.last'),
  );

  const heading = $derived(flow ? t('nowPlaying.flow', { variant: variantName(flow) }) : t('queue.title'));

  function variantName(variant: FlowVariant): string {
    if (variant.kind === 'genre') return variant.genre;
    return t(variant.kind === 'favorites' ? 'nowPlaying.flowFavorites' : 'nowPlaying.flowLibrary');
  }

  // The cache: one slot per queue entry. Pages remember the version they were read at.
  let cache: (QueueItem | undefined)[] = [];
  const fetched = new Map<number, number>();
  const pending = new Set<string>();
  let loaded = $state(0);
  /** Set by a move or removal made here: the version bump it causes keeps the cache. */
  let ownChange = false;
  let seenVersion = -1;

  $effect(() => {
    const version = player.queueVersion;
    untrack(() => {
      if (version === seenVersion) return;
      seenVersion = version;
      if (!ownChange) {
        cache = [];
        loaded++;
      }
      ownChange = false;
      fetched.clear();
    });
  });

  let scroller = $state<HTMLDivElement>();
  let listEl = $state<HTMLDivElement>();
  let scrollTop = $state(0);
  let viewport = $state(0);
  let selected = $state(-1);

  const range = $derived(visibleRange(scrollTop, viewport, ROW, count, 6));

  function ensure() {
    if (!open || empty) return;
    const version = player.queueVersion;
    const needed = pagesFor(start + range.start, start + range.end, PAGE);
    for (const page of needed) {
      const key = `${version}:${page}`;
      if (fetched.get(page) === version || pending.has(key)) continue;
      pending.add(key);
      playerQueueRange(page * PAGE, PAGE)
        .then((items) => {
          if (version !== player.queueVersion) return;
          for (const item of items ?? []) cache[item.index] = item;
          fetched.set(page, version);
          const keep = pagesFor(start + range.start, start + range.end, PAGE);
          for (const old of evictPages(fetched.keys(), keep, MAX_PAGES)) {
            fetched.delete(old);
            for (let i = old * PAGE; i < (old + 1) * PAGE; i++) cache[i] = undefined;
          }
          loaded++;
        })
        .catch((err) => console.error('loading up next failed', err))
        .finally(() => pending.delete(key));
    }
  }

  $effect(() => {
    void open;
    void player.queueVersion;
    void range.start;
    void range.end;
    void start;
    const frame = requestAnimationFrame(ensure);
    return () => cancelAnimationFrame(frame);
  });

  const rows = $derived.by(() => {
    void loaded;
    const out: { index: number; item: QueueItem | undefined }[] = [];
    for (let r = range.start; r < range.end; r++) out.push({ index: start + r, item: cache[start + r] });
    return out;
  });

  const activeId = $derived(
    selected >= start + range.start && selected < start + range.end ? `upnext-${selected}` : undefined,
  );

  /** Showing the played ones keeps the playing track near the top, with the last played one
   *  peeking out above it. */
  function setShowPlayed(on: boolean) {
    showPlayed = on;
    requestAnimationFrame(() => {
      if (!scroller) return;
      scroller.scrollTop = on ? Math.max(0, Math.round((current - 0.6) * ROW)) : 0;
      scrollTop = scroller.scrollTop;
    });
  }

  function onscroll() {
    if (scroller) scrollTop = scroller.scrollTop;
  }

  function report(what: string) {
    return (err: unknown) => console.error(`${what} failed`, err);
  }

  function jump(index: number) {
    playerJump(index).catch(report('jumping in the queue'));
  }

  function canRemove(index: number): boolean {
    return index !== current && index >= 0 && index < total;
  }

  function remove(index: number) {
    if (!canRemove(index)) return;
    ownChange = true;
    cache.splice(index, 1);
    loaded++;
    // The selection stays on its track, or moves to the one that took the removed place.
    if (selected > index || selected >= total - 1) selected--;
    playerRemove(index).catch(report('removing from the queue'));
  }

  /** Upcoming entries only: what played and what plays stay where they are. */
  function move(from: number, to: number) {
    if (from === to || from <= current || to <= current || to >= total) return;
    ownChange = true;
    const [item] = cache.splice(from, 1);
    cache.splice(to, 0, item);
    loaded++;
    selected = to;
    playerMove(from, to).catch(report('moving in the queue'));
  }

  function keepVisible(i: number) {
    if (!scroller) return;
    const top = (i - start) * ROW;
    if (top < scroller.scrollTop) scroller.scrollTop = top;
    else if (top + ROW > scroller.scrollTop + viewport) scroller.scrollTop = top + ROW - viewport;
  }

  /** Escape closes the panel from anywhere inside it. */
  function onPanelKeydown(event: KeyboardEvent) {
    if (event.key !== 'Escape' || event.defaultPrevented) return;
    event.preventDefault();
    toggleQueuePanel(false);
  }

  // Focus follows the panel: opening it (the button or Ctrl+Q) moves focus onto the playing
  // row; closing it while focus is inside hands focus back to the transport's toggle before
  // the panel turns inert, so it never falls to the page. A panel that is open at start (as
  // it was left) does not take focus.
  let panelEl = $state<HTMLElement>();
  let wasOpen = queuePanel.open;

  $effect.pre(() => {
    const now = open;
    untrack(() => {
      if (now === wasOpen) return;
      wasOpen = now;
      if (now) {
        void tick().then(focusInside);
      } else if (panelEl?.contains(document.activeElement)) {
        document.getElementById(QUEUE_TOGGLE_ID)?.focus();
      }
    });
  });

  function focusInside() {
    if (!queuePanel.open) return;
    if (listEl) {
      if (count > 0) selected = current;
      listEl.focus({ preventScroll: true });
      keepVisible(current);
    } else {
      panelEl?.querySelector<HTMLElement>('.close')?.focus();
    }
  }

  function onkeydown(event: KeyboardEvent) {
    if (count === 0) return;
    const step = Math.max(1, Math.floor(viewport / ROW) - 1);
    const from = selected >= start ? selected : current;
    if (event.altKey && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) {
      event.preventDefault();
      move(from, from + (event.key === 'ArrowUp' ? -1 : 1));
      keepVisible(selected);
      return;
    }
    const moves: Record<string, number> = {
      ArrowDown: from + 1,
      ArrowUp: from - 1,
      PageDown: from + step,
      PageUp: from - step,
      Home: start,
      End: total - 1,
    };
    if (event.key in moves) {
      event.preventDefault();
      selected = Math.max(start, Math.min(total - 1, moves[event.key]));
      keepVisible(selected);
    } else if (event.key === 'Enter' && selected >= 0) {
      event.preventDefault();
      jump(selected);
    } else if ((event.key === 'Delete' || event.key === 'Backspace') && canRemove(selected)) {
      event.preventDefault();
      remove(selected);
    } else if (isFavoriteKey(event) && selected >= 0) {
      const item = cache[selected]?.track;
      if (!item || item.id.startsWith('file:')) return;
      event.preventDefault();
      if (isLive(item)) {
        const station = findStation(item.id);
        if (station) void toggleStationFavorite(station);
      } else void toggleFavorite('track', item.id);
    }
  }

  // Dragging: the pressed row follows the pointer, the rows it passes step aside.
  let press: { index: number; y: number; scroll: number; pointer: number } | null = null;
  let drag = $state<QueueDrag | null>(null);
  let pointerY = 0;
  let frame = 0;
  let dragged = false;

  function onpointerdown(event: PointerEvent) {
    dragged = false;
    if (event.button !== 0 || (event.target as HTMLElement).closest('button')) return;
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-index]');
    if (!row || !scroller) return;
    const index = Number(row.dataset.index);
    if (index <= current) return;
    press = { index, y: event.clientY, scroll: scroller.scrollTop, pointer: event.pointerId };
    pointerY = event.clientY;
  }

  function follow() {
    if (!press || !scroller) return;
    const raw = pointerY - press.y + (scroller.scrollTop - press.scroll);
    const min = current + 1;
    const dy = Math.max((min - press.index) * ROW, Math.min((total - 1 - press.index) * ROW, raw));
    drag = { from: press.index, to: dragTarget(press.index, dy, ROW, min, total - 1), dy };
  }

  function autoscroll() {
    if (!drag || !scroller) return;
    const box = scroller.getBoundingClientRect();
    let step = 0;
    if (pointerY < box.top + SCROLL_EDGE) step = -Math.ceil((box.top + SCROLL_EDGE - pointerY) / 5);
    else if (pointerY > box.bottom - SCROLL_EDGE) step = Math.ceil((pointerY - (box.bottom - SCROLL_EDGE)) / 5);
    if (step) {
      scroller.scrollTop += step;
      follow();
    }
    frame = requestAnimationFrame(autoscroll);
  }

  function onpointermove(event: PointerEvent) {
    if (!press || event.pointerId !== press.pointer) return;
    pointerY = event.clientY;
    if (!drag) {
      if (Math.abs(event.clientY - press.y) < DRAG_START) return;
      try {
        listEl?.setPointerCapture?.(event.pointerId);
      } catch {
        // The pointer is already gone; the drag still ends on pointerup.
      }
      selected = press.index;
      frame = requestAnimationFrame(autoscroll);
    }
    follow();
  }

  function endDrag(apply: boolean) {
    cancelAnimationFrame(frame);
    if (drag) {
      const { from, to } = drag;
      drag = null;
      dragged = true;
      if (apply) move(from, to);
    }
    press = null;
  }

  function rowIndex(event: MouseEvent): number | null {
    if ((event.target as HTMLElement).closest('button')) return null;
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-index]');
    return row ? Number(row.dataset.index) : null;
  }
</script>

<!-- Escape bubbles up from the controls inside; the aside itself takes no input. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<aside
  class="panel"
  class:open
  aria-label={t('queue.title')}
  inert={!open}
  bind:this={panelEl}
  aria-keyshortcuts="Escape"
  onkeydown={onPanelKeydown}
  style="--c1:{ring.colors[0]};--c2:{ring.colors[1]};--c3:{ring.colors[2]};--c4:{ring.colors[3]}"
>
  <header class="head">
    <div class="titles">
      <h2 id="upnext-heading" class:flow={!!flow}>{heading}</h2>
      {#if !yt && !empty && !flow && !live}
        <p class="sub">{tc('queue.count', upcoming)}</p>
      {/if}
    </div>
    <button type="button" class="close" aria-label={t('queue.close')} title={t('queue.close')} onclick={() => toggleQueuePanel(false)}>
      <Icon name="close" size={16} />
    </button>
  </header>

  {#if yt}
    <div class="empty">
      <span class="empty-art" aria-hidden="true"><Icon name="queue" size={28} /></span>
      <p>{t('ytm.queueNote')}</p>
    </div>
  {:else if empty}
    <div class="empty">
      <span class="empty-art"><EmptyArt kind="queue" width={112} /></span>
      <strong>{t('queue.emptyTitle')}</strong>
      <p>{t('queue.emptyLead')}</p>
    </div>
  {:else}
    {#if current > 0}
      <button type="button" class="played" aria-expanded={showPlayed} onclick={() => setShowPlayed(!showPlayed)}>
        <span class="chev" class:up={showPlayed}><Icon name="chevronDown" size={14} /></span>
        {showPlayed ? t('queue.playedHide') : t('queue.playedShow', { count: current })}
      </button>
    {/if}

    <div class="scroll" bind:this={scroller} bind:clientHeight={viewport} {onscroll}>
      <div
        class="list"
        class:dragging={!!drag}
        bind:this={listEl}
        role="listbox"
        tabindex="0"
        aria-labelledby="upnext-heading"
        aria-activedescendant={activeId}
        aria-keyshortcuts="{FAVORITE_KEY} Delete Alt+ArrowUp Alt+ArrowDown Escape"
        style="height:{count * ROW}px"
        {onkeydown}
        {onpointerdown}
        {onpointermove}
        onpointerup={() => endDrag(true)}
        onpointercancel={() => endDrag(false)}
        onlostpointercapture={() => endDrag(true)}
        onclick={(event) => {
          if (dragged) {
            dragged = false;
            return;
          }
          const i = rowIndex(event);
          if (i === null) return;
          selected = i;
          if (i !== current) jump(i);
        }}
      >
        <div class="window" style="transform:translateY({range.start * ROW}px)">
          {#each rows as row (row.index)}
            {@const item = row.item}
            {@const isCurrent = row.index === current}
            {@const name = item?.track?.title ?? ''}
            <div
              id="upnext-{row.index}"
              class="row"
              class:current={isCurrent}
              class:past={row.index < current}
              class:upcoming={row.index > current}
              class:selected={selected === row.index}
              class:lifted={drag?.from === row.index}
              style:transform={drag ? `translateY(${stepAside(row.index, drag, ROW)}px)` : null}
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
                    {name}
                    current={isCurrent}
                    revealed={selected === row.index}
                    suppressed={!!drag}
                    onplay={() => {
                      selected = row.index;
                      jump(row.index);
                    }}
                  />
                {:else}
                  {row.index + 1}
                {/if}
              </span>
              {#if item?.track}
                {@const q = item.track}
                {#if isLive(q)}
                  <StationTile name={q.title} coverId={q.coverId} size={40} radius="7px" />
                {:else}
                  <Cover coverId={q.coverId} size={40} cache={96} radius="7px" />
                {/if}
                <span class="name">
                  <span class="title">{q.title}</span>
                  <span class="artist">{isCurrent ? t('queue.nowPlaying') : q.artist}</span>
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
                <span class="name"><span class="artist">{t('queue.missing')}</span></span>
                <span></span>
                <span></span>
              {:else}
                <span class="ghost-cover"></span>
                <span class="name"><span class="ghost" style="width:{40 + ((row.index * 37) % 35)}%"></span></span>
                <span></span>
                <span></span>
              {/if}
              {#if canRemove(row.index)}
                <button
                  type="button"
                  class="remove"
                  class:shown={selected === row.index}
                  tabindex="-1"
                  aria-label={t('queue.remove', { name: name || t('queue.missing') })}
                  title={t('queue.remove', { name: name || t('queue.missing') })}
                  onmousedown={(event) => event.preventDefault()}
                  onclick={() => remove(row.index)}
                >
                  <Icon name="close" size={14} />
                </button>
              {:else}
                <span></span>
              {/if}
            </div>
          {/each}
        </div>
      </div>
      {#if upcoming === 0 && !flow && !live}
        <p class="tail">{tailNote}</p>
      {/if}
    </div>
  {/if}
</aside>

<style>
  /* A sheet floating over the content's right side. It slides in and out over 220 ms; the
     owner asked for this motion to stay smooth under reduced motion too, so it does not read
     --dur-l. */
  .panel {
    position: absolute;
    top: calc(var(--header-h) - 4px);
    right: 12px;
    bottom: 14px;
    z-index: 4;
    width: 372px;
    max-width: calc(100% - 24px);
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    padding: 18px 10px 0 14px;
    border-radius: var(--radius-l);
    background:
      radial-gradient(120% 40% at 100% 0%, color-mix(in srgb, var(--c4) 7%, transparent), transparent 70%),
      rgba(25, 25, 31, 0.94);
    backdrop-filter: blur(22px) saturate(1.15);
    box-shadow:
      inset 0 0 0 1px rgba(255, 255, 255, 0.08),
      inset 0 1px 0 rgba(255, 255, 255, 0.05),
      -24px 28px 60px -20px rgba(0, 0, 0, 0.85);
    transform: translateX(calc(100% + 24px));
    visibility: hidden;
    transition:
      transform 220ms cubic-bezier(0.22, 0.8, 0.24, 1),
      visibility 0s linear 220ms;
  }

  .panel.open {
    transform: none;
    visibility: visible;
    transition:
      transform 220ms cubic-bezier(0.22, 0.8, 0.24, 1),
      visibility 0s;
  }

  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 0 4px 12px 4px;
  }

  .titles {
    min-width: 0;
  }

  h2 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 19px;
    font-weight: 600;
    letter-spacing: -0.015em;
    line-height: 1.2;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* In Flow the heading carries the prism's spectrum, as in Now Playing. */
  h2.flow {
    background: linear-gradient(90deg, var(--spec-1), var(--spec-2) 35%, var(--spec-3) 68%, var(--spec-4));
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }

  .sub {
    margin: 3px 0 0;
    font-size: 12.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .close {
    flex: none;
    width: 30px;
    height: 30px;
    margin: -3px -2px 0 0;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--text-dim);
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .close:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
  }

  .played {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    margin: 0 0 8px 2px;
    padding: 0 10px 0 7px;
    border: 0;
    border-radius: 13px;
    background: rgba(255, 255, 255, 0.05);
    color: var(--text-dim);
    font: inherit;
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .played:hover {
    background: rgba(255, 255, 255, 0.09);
    color: var(--text);
  }

  .chev {
    display: grid;
    transition: transform var(--dur) ease-out;
  }

  .chev.up {
    transform: rotate(180deg);
  }

  .close:focus-visible,
  .played:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 1px;
  }

  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    margin-right: -4px;
    padding: 2px 4px 18px 0;
    mask-image: linear-gradient(to bottom, #000 calc(100% - 24px), transparent);
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
    position: relative;
    display: grid;
    grid-template-columns: 26px 40px minmax(0, 1fr) 26px minmax(38px, auto) 24px;
    column-gap: 10px;
    align-items: center;
    height: 56px;
    padding: 0 6px 0 8px;
    box-sizing: border-box;
    border-radius: 11px;
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      opacity var(--dur) ease-out;
  }

  .row.upcoming {
    cursor: grab;
  }

  .row:hover {
    background: rgba(255, 255, 255, 0.05);
  }

  .row.selected {
    background: rgba(255, 255, 255, 0.075);
  }

  .row.past {
    opacity: 0.46;
  }

  .row.past:hover,
  .row.past.selected {
    opacity: 0.85;
  }

  /* The playing track glows with its own cover's colors, edged with the prism's spectrum. */
  .row.current {
    background:
      linear-gradient(90deg, color-mix(in srgb, var(--c1) 26%, transparent), color-mix(in srgb, var(--c2) 10%, transparent) 55%, transparent),
      rgba(255, 255, 255, 0.06);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.07);
    cursor: default;
  }

  .row.current::before {
    content: '';
    position: absolute;
    left: 0;
    top: 10px;
    bottom: 10px;
    width: 3px;
    border-radius: 3px;
    background: linear-gradient(180deg, var(--spec-1), var(--spec-2), var(--spec-3), var(--spec-4));
  }

  .row:hover :global(.heart.hidden) {
    opacity: 1;
    pointer-events: auto;
  }

  .list:focus-visible .row.selected {
    box-shadow: inset 0 0 0 1.5px rgba(77, 166, 255, 0.7);
  }

  .list.dragging {
    cursor: grabbing;
  }

  .list.dragging .row {
    transition: transform 180ms cubic-bezier(0.2, 0.7, 0.2, 1);
  }

  .list.dragging .row:hover {
    background: transparent;
  }

  .list.dragging .row.lifted {
    z-index: 2;
    transition: none;
    background: var(--raised);
    box-shadow:
      inset 0 0 0 1px rgba(255, 255, 255, 0.1),
      0 18px 36px -14px rgba(0, 0, 0, 0.9);
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

  .title,
  .artist {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .title {
    font-size: 13.5px;
    font-weight: 500;
  }

  .artist {
    font-size: 12px;
    color: var(--text-dim);
  }

  .current .title {
    color: #fff;
    font-weight: 600;
  }

  .remove {
    width: 24px;
    height: 24px;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--text-dim);
    cursor: pointer;
    opacity: 0;
    pointer-events: none;
    transition:
      opacity var(--dur) ease-out,
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .row:hover .remove,
  .remove.shown {
    opacity: 1;
    pointer-events: auto;
  }

  .list.dragging .remove {
    opacity: 0;
    pointer-events: none;
  }

  .remove:hover {
    background: rgba(255, 77, 109, 0.16);
    color: var(--spec-1);
  }

  .ghost-cover,
  .ghost {
    display: block;
    height: 10px;
    border-radius: 5px;
    background: rgba(255, 255, 255, 0.05);
  }

  .ghost-cover {
    width: 40px;
    height: 40px;
    border-radius: 7px;
  }

  .tail {
    margin: 14px 8px 0;
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--text-dim);
  }

  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 0 28px 48px;
    text-align: center;
  }

  .empty-art {
    display: grid;
    place-items: center;
    margin-bottom: 6px;
  }

  .empty strong {
    font-family: var(--font-display);
    font-size: 17px;
    font-weight: 600;
  }

  .empty p {
    margin: 0;
    font-size: 13px;
    line-height: 1.55;
    color: var(--text-dim);
  }
</style>
