<script lang="ts">
  import { tick } from 'svelte';
  import { flip } from 'svelte/animate';
  import { formatDuration } from '../format';
  import { t, tc } from '../i18n';
  import { coverUrl, playlistDelete, playlistEntries, playlistMove, playlistRemove } from '../ipc';
  import { menuPoint } from '../menu';
  import { playPlaylist } from '../play';
  import { FAVORITE_KEY, isFavoriteKey, toggleFavorite } from '../stores/favorites.svelte';
  import { library } from '../stores/library.svelte';
  import { back, go } from '../stores/nav.svelte';
  import { notify } from '../stores/notice.svelte';
  import { player } from '../stores/player.svelte';
  import { playlists, renamePlaylist } from '../stores/playlists.svelte';
  import type { PlaylistEntry } from '../types';
  import Backdrop from './Backdrop.svelte';
  import Cover from './Cover.svelte';
  import Heart from './Heart.svelte';
  import Icon from './Icon.svelte';
  import NowPlayingBars from './NowPlayingBars.svelte';
  import PlaylistCover from './PlaylistCover.svelte';
  import TrackMenu, { type TrackMenuState } from './TrackMenu.svelte';

  /** One playlist: its mosaic over a blurred backdrop, a title that is edited in place, play and
   *  shuffle, and the entries. Rows are dragged to a new place (or moved with Alt+Up and
   *  Alt+Down) and removed with Delete or the menu. Entries whose track is not in the library
   *  stay in place, dimmed, and are skipped when playing. */
  let { playlistId }: { playlistId: number } = $props();

  /** Row height; dragging moves rows in whole rows. */
  const ROW = 54;
  /** A press turns into a drag after this many pixels. */
  const DRAG_START = 5;
  /** Near the top or bottom edge of the view, a drag scrolls it. */
  const SCROLL_EDGE = 56;

  let entries = $state.raw<PlaylistEntry[] | null>(null);
  let failed = $state<string | null>(null);
  let selected = $state(-1);
  /** Bumped to refetch the entries after a change the backend refused. */
  let refresh = $state(0);
  let menu = $state<TrackMenuState | null>(null);

  $effect(() => {
    const id = playlistId;
    void playlists.version;
    void library.version;
    void refresh;
    let gone = false;
    const asked = changes;
    playlistEntries(id)
      .then((list) => {
        // A change made since the request (or still on its way) would be undone by this
        // list; the queue refetches once it has drained.
        if (gone || pending > 0 || asked !== changes) return;
        entries = list ?? [];
        failed = null;
      })
      .catch((err) => {
        if (!gone) failed = String(err);
      });
    return () => {
      gone = true;
    };
  });

  const playlist = $derived(playlists.list?.find((p) => p.id === playlistId) ?? null);
  const missingPlaylist = $derived(playlists.list !== null && !playlist);
  const name = $derived(playlist?.name ?? '');
  const count = $derived(entries?.length ?? playlist?.trackCount ?? 0);
  const unavailable = $derived(entries?.filter((entry) => !entry.track).length ?? 0);
  const playable = $derived(count - unavailable);
  const totalMs = $derived(entries?.reduce((sum, entry) => sum + (entry.track?.durationMs ?? 0), 0) ?? 0);
  const coverIds = $derived(playlist?.coverIds ?? []);
  const backdrop = $derived(coverUrl(coverIds[0] ?? null, 96));
  const playingId = $derived(player.track?.id ?? null);

  /** Keys that survive a reorder: the track id, numbered when it appears more than once. */
  const rows = $derived.by(() => {
    const seen = new Map<string, number>();
    return (entries ?? []).map((entry, i) => {
      const n = seen.get(entry.trackId) ?? 0;
      seen.set(entry.trackId, n + 1);
      return { entry, i, key: `${entry.trackId}#${n}` };
    });
  });

  function length(ms: number): string {
    const minutes = Math.max(1, Math.round(ms / 60000));
    if (minutes < 60) return t('time.minutes', { m: minutes });
    return t('time.hours', { h: Math.floor(minutes / 60), m: minutes % 60 });
  }

  // Title

  let editing = $state(false);
  let draft = $state('');
  let titleButton = $state<HTMLButtonElement>();

  function startEdit() {
    if (!playlist) return;
    draft = playlist.name;
    editing = true;
  }

  async function commit() {
    if (!editing) return;
    editing = false;
    await renamePlaylist(playlistId, draft);
  }

  function onTitleKey(event: KeyboardEvent) {
    if (event.key === 'Enter') {
      event.preventDefault();
      void commit();
      tick().then(() => titleButton?.focus());
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      editing = false;
      tick().then(() => titleButton?.focus());
    }
  }

  // A playlist just made from the Playlists view opens with its title ready to type over.
  $effect(() => {
    if (playlist && playlists.renaming === playlistId) {
      playlists.renaming = null;
      startEdit();
    }
  });

  // Delete

  let confirming = $state(false);

  async function deletePlaylist() {
    confirming = false;
    try {
      await playlistDelete(playlistId);
      playlists.list = (playlists.list ?? []).filter((p) => p.id !== playlistId);
      go('playlists');
    } catch (err) {
      notify(t('playlist.deleteFailed', { message: String(err) }));
    }
  }

  // Playing

  /** Plays from row `i`; a dimmed row (not in the library) does nothing. */
  function play(i: number) {
    const entry = entries?.[i];
    if (entry?.track) void playPlaylist(playlistId, entry.position);
  }

  // Changing the order and removing. Both show at once and go to the backend one after the
  // other, each computed against the list as the ones before it left it. While any is on its
  // way, fetched lists are ignored; once the queue has drained the entries are fetched once.

  /** Changes sent or waiting. */
  let pending = 0;
  /** Bumped by every change, so a fetch started before it is recognized as stale. */
  let changes = 0;
  let queue: Promise<void> = Promise.resolve();

  function enqueue(send: () => Promise<void>) {
    pending++;
    changes++;
    queue = queue
      .then(send)
      .catch((err) => notify(t('playlist.changeFailed', { message: String(err) })))
      .finally(() => {
        pending--;
        if (pending === 0) refresh++;
      });
  }

  function move(from: number, to: number) {
    if (!entries || from === to || !entries[from] || to < 0 || to >= entries.length) return;
    const next = [...entries];
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    entries = next.map((entry, position) => ({ ...entry, position }));
    selected = to;
    const id = playlistId;
    enqueue(() => playlistMove(id, from, to));
  }

  function removeAt(i: number) {
    const entry = entries?.[i];
    if (!entries || !entry) return;
    entries = entries.filter((_, k) => k !== i).map((e, position) => ({ ...e, position }));
    if (selected >= entries.length) selected = entries.length - 1;
    const id = playlistId;
    enqueue(() => playlistRemove(id, [entry.position]));
  }

  // The list: selection, keys, menu.

  let scroller = $state<HTMLDivElement>();
  let listEl = $state<HTMLDivElement>();

  function reveal(i: number) {
    tick().then(() => document.getElementById(`pl-row-${i}`)?.scrollIntoView({ block: 'nearest' }));
  }

  function rowIndex(event: MouseEvent): number | null {
    const target = event.target as HTMLElement;
    if (target.closest('button')) return null;
    const row = target.closest<HTMLElement>('[data-index]');
    return row ? Number(row.dataset.index) : null;
  }

  function onkeydown(event: KeyboardEvent) {
    const n = entries?.length ?? 0;
    if (!n) return;
    if (event.altKey && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) {
      event.preventDefault();
      if (selected < 0) return;
      const to = selected + (event.key === 'ArrowUp' ? -1 : 1);
      if (to < 0 || to >= n) return;
      move(selected, to);
      reveal(to);
      return;
    }
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const moves: Record<string, number> = { ArrowDown: selected + 1, ArrowUp: selected - 1, Home: 0, End: n - 1 };
    if (event.key in moves) {
      event.preventDefault();
      selected = Math.max(0, Math.min(n - 1, moves[event.key]));
      reveal(selected);
    } else if (event.key === 'Enter' && selected >= 0) {
      event.preventDefault();
      play(selected);
    } else if (event.key === 'Delete' && selected >= 0) {
      event.preventDefault();
      removeAt(selected);
    } else if (isFavoriteKey(event) && entries?.[selected]?.track) {
      event.preventDefault();
      void toggleFavorite('track', entries[selected].trackId);
    }
  }

  function oncontextmenu(event: MouseEvent) {
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-index]');
    const i = row ? Number(row.dataset.index) : selected;
    const entry = entries?.[i];
    if (!entry) return;
    event.preventDefault();
    selected = i;
    const track = entry.track;
    const at = menuPoint(event, document.getElementById(`pl-row-${i}`));
    menu = {
      trackIds: [entry.trackId],
      ...at,
      albumId: track?.albumId,
      artist: track ? track.albumArtist || track.artist : undefined,
      onremove: () => {
        // The entry may have moved while the menu was open: find it by its key.
        const key = rows[i]?.key;
        const now = rows.findIndex((r) => r.key === key);
        removeAt(now >= 0 ? now : i);
      },
    };
  }

  // Dragging. The pressed row follows the pointer; the rows it passes step aside by one row.

  let press: { index: number; y: number; scroll: number; pointer: number } | null = null;
  let drag = $state<{ from: number; to: number; dy: number } | null>(null);
  let pointerY = 0;
  let frame = 0;
  let dragged = false;

  function onpointerdown(event: PointerEvent) {
    dragged = false;
    if (event.button !== 0 || (event.target as HTMLElement).closest('button')) return;
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-index]');
    if (!row || !scroller) return;
    press = { index: Number(row.dataset.index), y: event.clientY, scroll: scroller.scrollTop, pointer: event.pointerId };
    pointerY = event.clientY;
  }

  function follow() {
    if (!press || !scroller || !entries) return;
    const n = entries.length;
    const raw = pointerY - press.y + (scroller.scrollTop - press.scroll);
    const dy = Math.max(-press.index * ROW, Math.min((n - 1 - press.index) * ROW, raw));
    drag = { from: press.index, to: Math.max(0, Math.min(n - 1, press.index + Math.round(dy / ROW))), dy };
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
      menu = null;
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

  /** Where row `i` is drawn while a drag is on. */
  function offset(i: number): number {
    if (!drag) return 0;
    const { from, to, dy } = drag;
    if (i === from) return dy;
    if (from < to && i > from && i <= to) return -ROW;
    if (to < from && i >= to && i < from) return ROW;
    return 0;
  }

  const reduced = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;
  const settle = reduced ? 0 : 200;
</script>

<div class="scroll" bind:this={scroller}>
  {#if backdrop}
    <Backdrop src={backdrop} height={520} />
  {/if}

  <div class="inner">
    <button type="button" class="back" onclick={back} aria-label={t('album.back')} title={t('album.back')}>
      <Icon name="back" size={18} />
    </button>

    {#if missingPlaylist}
      <p class="note">{t('playlist.gone')}</p>
    {:else if playlist}
      <header class="hero">
        <div class="art">
          <PlaylistCover {coverIds} seed={playlist.id} size={232} radius="14px" cache={600} />
        </div>
        <div class="info">
          {#if editing}
            <input
              class="title-input"
              type="text"
              bind:value={draft}
              maxlength="200"
              spellcheck="false"
              aria-label={t('playlists.nameLabel')}
              onkeydown={onTitleKey}
              onblur={commit}
              {@attach (node) => {
                node.focus();
                node.select();
              }}
            />
          {:else}
            <h1>
              <button
                bind:this={titleButton}
                type="button"
                class="rename"
                title={t('playlist.rename')}
                aria-describedby="pl-rename-hint"
                onclick={startEdit}
              >
                <span class="text">{name}</span>
                <span class="pen" aria-hidden="true"><Icon name="edit" size={18} /></span>
              </button>
            </h1>
            <span id="pl-rename-hint" class="sr-only">{t('playlist.rename')}</span>
          {/if}
          <p class="meta">
            <span>{tc('view.count.songs', count)}</span>
            {#if totalMs > 0}<span>{length(totalMs)}</span>{/if}
            {#if unavailable}<span class="warn">{t('playlist.unavailable', { count: unavailable })}</span>{/if}
          </p>
          <div class="actions">
            <button type="button" class="btn primary" disabled={!playable} onclick={() => playPlaylist(playlistId)}>
              <Icon name="play" size={16} filled />
              {t('album.play')}
            </button>
            <button
              type="button"
              class="btn ghost"
              disabled={!playable}
              onclick={() => playPlaylist(playlistId, 0, { shuffled: true })}
            >
              <Icon name="shuffle" size={16} />
              {t('album.shuffle')}
            </button>
            {#if confirming}
              <span class="confirm">
                <button type="button" class="btn danger" onclick={deletePlaylist}>{t('playlist.deleteConfirm')}</button>
                <button type="button" class="btn ghost" onclick={() => (confirming = false)}>{t('playlist.keep')}</button>
              </span>
            {:else}
              <button
                type="button"
                class="round"
                aria-label={t('playlist.delete')}
                title={t('playlist.delete')}
                onclick={() => (confirming = true)}
              >
                <Icon name="trash" size={17} />
              </button>
            {/if}
          </div>
        </div>
      </header>

      {#if failed}
        <p class="note">{failed}</p>
      {:else if entries && entries.length === 0}
        <div class="empty">
          <h2>{t('playlist.empty')}</h2>
          <p class="lead">{t('playlist.emptyLead')}</p>
          <button type="button" class="btn ghost" onclick={() => go('albums')}>
            <Icon name="album" size={16} />
            {t('favorites.browse')}
          </button>
        </div>
      {:else if entries}
        <div
          bind:this={listEl}
          class="list"
          class:dragging={!!drag}
          role="listbox"
          tabindex="0"
          aria-label={name}
          aria-keyshortcuts="{FAVORITE_KEY} Delete Alt+ArrowUp Alt+ArrowDown"
          aria-activedescendant={selected >= 0 && selected < entries.length ? `pl-row-${selected}` : undefined}
          {onkeydown}
          {oncontextmenu}
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
            if (i !== null) selected = i;
          }}
          ondblclick={(event) => {
            const i = rowIndex(event);
            if (i !== null) play(i);
          }}
        >
          {#each rows as row (row.key)}
            {@const track = row.entry.track}
            {@const playing = !!track && track.id === playingId}
            <div
              id="pl-row-{row.i}"
              class="row"
              class:selected={selected === row.i}
              class:playing
              class:missing={!track}
              class:lifted={drag?.from === row.i}
              role="option"
              aria-selected={selected === row.i}
              aria-disabled={!track || undefined}
              data-index={row.i}
              style:transform={drag ? `translateY(${offset(row.i)}px)` : null}
              animate:flip={{ duration: settle }}
            >
              <span class="num">
                {#if playing}
                  <NowPlayingBars playing={player.status === 'playing'} size={12} />
                {:else}
                  <span class="n">{row.i + 1}</span>
                  <span class="grip" title={t('playlist.reorder')}><Icon name="grip" size={16} /></span>
                {/if}
              </span>
              <Cover coverId={track?.coverId ?? null} size={38} cache={96} radius="7px" />
              <span class="name">
                <span class="title">{track ? track.title : t('playlist.missing')}</span>
                {#if track}
                  <button
                    type="button"
                    class="sub link"
                    tabindex="-1"
                    onclick={() => go('artist', track.albumArtist || track.artist)}
                  >
                    {track.artist}
                  </button>
                {/if}
              </span>
              {#if track}
                <button type="button" class="album link" tabindex="-1" onclick={() => go('album', track.albumId)}>
                  {track.album}
                </button>
                <Heart
                  kind="track"
                  id={track.id}
                  size={16}
                  tabindex={-1}
                  ghost
                  revealed={selected === row.i}
                  label={t('favorites.toggleItem', { name: track.title })}
                />
                <span class="time">{formatDuration(track.durationMs)}</span>
              {:else}
                <span></span>
                <span></span>
                <span></span>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    {/if}
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
  }

  .inner {
    position: relative;
    padding: 0 32px 40px;
  }

  .back {
    width: 32px;
    height: 32px;
    display: grid;
    place-items: center;
    margin: 0 0 14px -6px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: rgba(0, 0, 0, 0.22);
    color: var(--text);
    cursor: pointer;
    transition: background-color var(--dur) ease-out;
  }

  .back:hover {
    background: rgba(0, 0, 0, 0.4);
  }

  .back:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
  }

  .hero {
    display: flex;
    align-items: flex-end;
    gap: 32px;
    margin-bottom: 34px;
  }

  .art {
    flex: none;
    border-radius: 14px;
    box-shadow:
      0 30px 60px -20px rgba(0, 0, 0, 0.85),
      0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  .info {
    flex: 1;
    min-width: 0;
    padding-bottom: 4px;
  }

  /* The title is the name and its editor: the same type before and while it is edited, so
     clicking it only adds a caret and an underline. */
  h1,
  .title-input {
    margin: 0 0 8px;
    font-family: var(--font-display);
    font-size: clamp(30px, 3.6vw, 46px);
    font-weight: 600;
    letter-spacing: -0.025em;
    line-height: 1.04;
  }

  /* Both are exactly one line box per line (1.04em): the title does not move when it turns
     into the field, and neither does the meta line under it. */
  h1 {
    display: flex;
    text-wrap: balance;
  }

  .rename {
    display: flex;
    line-height: inherit;
    align-items: flex-end;
    gap: 12px;
    max-width: 100%;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    letter-spacing: inherit;
    text-align: left;
    cursor: text;
    border-radius: 6px;
  }

  .rename .text {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    overflow-wrap: anywhere;
  }

  .pen {
    flex: none;
    display: grid;
    margin-bottom: 0.18em;
    color: var(--text-dim);
    opacity: 0;
    transition: opacity var(--dur) ease-out;
  }

  .rename:hover .pen,
  .rename:focus-visible .pen {
    opacity: 1;
  }

  .rename:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 4px;
  }

  .title-input {
    display: block;
    width: 100%;
    height: 1.04em;
    box-sizing: content-box;
    padding: 0;
    border: 0;
    /* The underline is a shadow, so it adds no height. */
    box-shadow: 0 2px 0 color-mix(in srgb, var(--spec-4) 70%, transparent);
    border-radius: 0;
    background: transparent;
    color: var(--text);
    caret-color: var(--spec-4);
    outline: none;
    user-select: text;
    -webkit-user-select: text;
  }

  .title-input::selection {
    background: color-mix(in srgb, var(--spec-4) 38%, transparent);
  }

  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 16px;
    margin: 10px 0 22px;
    font-size: 13px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .warn {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }

  .warn::before {
    content: '';
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--spec-2);
    box-shadow: 0 0 8px color-mix(in srgb, var(--spec-2) 55%, transparent);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
  }

  .confirm {
    display: inline-flex;
    gap: 8px;
    animation: fade var(--dur) ease-out;
  }

  .btn.danger {
    background: color-mix(in srgb, var(--spec-1) 22%, transparent);
    color: #fff;
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--spec-1) 55%, transparent);
    padding: 0 18px;
  }

  .btn.danger:hover {
    background: color-mix(in srgb, var(--spec-1) 34%, transparent);
  }

  .round {
    width: 38px;
    height: 38px;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.06);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.1);
    color: var(--text-dim);
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .round:hover {
    background: rgba(255, 255, 255, 0.1);
    color: var(--text);
  }

  .round:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
  }

  /* Rows */

  .list {
    outline: none;
    margin: 0 -12px;
  }

  .row {
    position: relative;
    display: grid;
    grid-template-columns: 28px 38px minmax(0, 1.2fr) minmax(0, 1fr) 30px 52px;
    column-gap: 14px;
    align-items: center;
    height: 54px;
    padding: 0 12px;
    border-radius: 10px;
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

  /* While a row is dragged the others step aside smoothly; the dragged one is lifted off the
     list and follows the pointer without lag. */
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
      inset 0 0 0 1px rgba(255, 255, 255, 0.09),
      0 18px 36px -14px rgba(0, 0, 0, 0.9);
  }

  @media (prefers-reduced-motion: reduce) {
    .list.dragging .row {
      transition: none;
    }
  }

  .num,
  .time {
    display: flex;
    justify-content: flex-end;
    font-size: 12.5px;
    color: rgba(243, 243, 245, 0.45);
    font-variant-numeric: tabular-nums;
  }

  .num {
    position: relative;
  }

  /* The grip takes the number's place on hover: rows are picked up anywhere, it shows how. */
  .grip {
    position: absolute;
    right: -3px;
    top: 50%;
    display: grid;
    transform: translateY(-50%);
    color: var(--text-dim);
    opacity: 0;
    cursor: grab;
    transition: opacity var(--dur) ease-out;
  }

  .row:hover .grip,
  .row.lifted .grip {
    opacity: 1;
  }

  .row:hover .n,
  .row.lifted .n {
    opacity: 0;
  }

  .name {
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
  }

  .title,
  .sub,
  .album {
    max-width: 100%;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .title {
    font-weight: 500;
  }

  .playing .title {
    color: #fff;
    font-weight: 600;
  }

  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--text-dim);
    font: inherit;
    text-align: left;
    cursor: pointer;
    justify-self: start;
  }

  .sub.link {
    font-size: 12.5px;
  }

  .link:hover {
    color: var(--text);
    text-decoration: underline;
    text-underline-offset: 3px;
    text-decoration-color: rgba(243, 243, 245, 0.35);
  }

  /* Not in the library: kept in its place, quiet, skipped when playing. */
  .row.missing > :global(*) {
    opacity: 0.42;
  }

  .row.missing .title {
    font-weight: 450;
    font-style: italic;
  }

  .note {
    color: var(--text-dim);
    font-size: 14px;
  }

  .empty {
    max-width: 420px;
    padding: 4px 0 12px;
  }

  h2 {
    margin: 0 0 8px;
    font-family: var(--font-display);
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .lead {
    margin: 0 0 20px;
    font-size: 14px;
    line-height: 1.55;
    color: var(--text-dim);
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  @media (max-width: 1000px) {
    .hero {
      gap: 24px;
    }
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }
</style>
