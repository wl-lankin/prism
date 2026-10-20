<script lang="ts">
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import About from './lib/components/About.svelte';
  import AlbumGrid from './lib/components/AlbumGrid.svelte';
  import AlbumView from './lib/components/AlbumView.svelte';
  import ArtistList from './lib/components/ArtistList.svelte';
  import ArtistView from './lib/components/ArtistView.svelte';
  import EmptyLibrary from './lib/components/EmptyLibrary.svelte';
  import FavoritesView from './lib/components/FavoritesView.svelte';
  import FlowMenu from './lib/components/FlowMenu.svelte';
  import FlowRing, { type RingMode } from './lib/components/FlowRing.svelte';
  import Icon from './lib/components/Icon.svelte';
  import SearchPalette from './lib/components/SearchPalette.svelte';
  import Settings from './lib/components/Settings.svelte';
  import Sidebar from './lib/components/Sidebar.svelte';
  import SongList from './lib/components/SongList.svelte';
  import TitleBar from './lib/components/TitleBar.svelte';
  import TransportStrip from './lib/components/TransportStrip.svelte';
  import ViewHead from './lib/components/ViewHead.svelte';
  import { activateIdle, holdToOpen } from './lib/flow';
  import { t, tc, type Key } from './lib/i18n';
  import { playerPlayFiles, playerToggle, startupNotice, windowVibrancy } from './lib/ipc';
  import { initFavorites } from './lib/stores/favorites.svelte';
  import { initLibrary, library } from './lib/stores/library.svelte';
  import { nav, overlays, type View } from './lib/stores/nav.svelte';
  import { notice, notify } from './lib/stores/notice.svelte';
  import { initPlayer, player } from './lib/stores/player.svelte';
  import { initSettings } from './lib/stores/settings.svelte';

  /** The Flow ring hangs half out below the app's bottom edge, into the transparent strip
   *  at the bottom of the window. */
  const RING = 96;
  const isMac = typeof navigator !== 'undefined' && /Mac/.test(navigator.platform || navigator.userAgent);

  let glass = $state(false);
  let starting = $state(false);
  let searching = $state(false);
  let dragging = $state(false);
  /** Where the Flow menu is open, if it is. */
  let flowMenu: { x: number; y: number } | null = $state(null);

  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    initPlayer()
      .then((unlisten) => (gone ? unlisten() : (stop = unlisten)))
      .catch((err) => console.error('connecting to the player failed', err));
    windowVibrancy()
      .then((on) => {
        glass = on;
        document.documentElement.dataset.vibrancy = on ? 'on' : 'off';
      })
      .catch(() => {});
    return () => {
      gone = true;
      stop?.();
    };
  });

  $effect(() => initLibrary());

  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    initFavorites()
      .then((cleanup) => (gone ? cleanup() : (stop = cleanup)))
      .catch((err) => console.error('loading the favorites failed', err));
    return () => {
      gone = true;
      stop?.();
    };
  });

  $effect(() => {
    startupNotice()
      .then((file) => {
        if (file) notify(t('startup.quarantined', { file }));
      })
      .catch(() => {});
  });

  $effect(() => {
    initSettings().catch((err) => console.error('loading the settings failed', err));
  });

  // Files dropped onto the window play at once, without being indexed.
  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    try {
      getCurrentWindow()
        .onDragDropEvent((event) => {
          const payload = event.payload;
          if (payload.type === 'enter') dragging = payload.paths.length > 0;
          else if (payload.type === 'leave') dragging = false;
          else if (payload.type === 'drop') {
            dragging = false;
            if (payload.paths.length) void playDropped(payload.paths);
          }
        })
        .then((unlisten) => (gone ? unlisten() : (stop = unlisten)))
        .catch(() => {});
    } catch {
      // Not inside Tauri (tests).
    }
    return () => {
      gone = true;
      stop?.();
    };
  });

  async function playDropped(paths: string[]) {
    try {
      if ((await playerPlayFiles(paths)) === 0) notify(t('drop.none'));
    } catch (err) {
      notify(t('drop.failed', { message: String(err) }));
    }
  }

  function onkeydown(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey && event.key.toLowerCase() === 'k') {
      event.preventDefault();
      searching = !searching;
    }
  }

  const mode: RingMode = $derived(
    player.status === 'idle' ? 'flow' : player.status === 'playing' ? 'pause' : 'play',
  );
  const progress = $derived(player.durationMs > 0 ? player.positionMs / player.durationMs : 0);

  async function activate() {
    if (player.status !== 'idle') {
      playerToggle().catch((err) => notify(t('flow.failed', { message: String(err) })));
      return;
    }
    if (starting) return;
    starting = true;
    try {
      // An endless Flow from the favorites (the whole library when there are none yet).
      await activateIdle();
    } finally {
      starting = false;
    }
  }

  let dock: HTMLDivElement | undefined = $state();

  /** Opens the Flow menu above the ring (centred on it), so the ring stays in view. */
  function openFlowMenu() {
    const box = dock?.getBoundingClientRect();
    if (box) flowMenu = { x: box.left + box.width / 2, y: box.top };
  }

  /** Views that show the library, and so the empty state when it has no tracks. */
  function libraryView(view: View): boolean {
    return view === 'home' || view === 'albums' || view === 'artists' || view === 'songs';
  }

  const titles: Record<View, Key> = {
    home: 'nav.home',
    favorites: 'nav.favorites',
    playlists: 'nav.playlists',
    albums: 'nav.albums',
    artists: 'nav.artists',
    songs: 'nav.songs',
    album: 'nav.albums',
    artist: 'nav.artists',
    settings: 'nav.settings',
  };
</script>

<svelte:window {onkeydown} />

{#snippet albumsHead()}
  <ViewHead title={t('nav.albums')} meta={tc('view.count.albums', library.albums?.length ?? 0)} />
{/snippet}

<div class="window" class:glass data-tauri-drag-region style="--ring-size:{RING}px">
  <!-- First in the DOM so the main control comes early in the tab order; placed by CSS. -->
  <div class="ring-dock" style="--ring:{RING}px" bind:this={dock} use:holdToOpen={openFlowMenu}>
    <div class="notice" class:shown={notice.text} role="status" aria-live="polite">{notice.text ?? ''}</div>
    <FlowRing
      {mode}
      {progress}
      size={RING}
      onactivate={activate}
      oncontextmenu={openFlowMenu}
    />
  </div>

  <div class="card">
    <Sidebar />

    <main class="main">
      <TitleBar>
        <button
          type="button"
          class="search"
          onclick={() => (searching = true)}
          aria-keyshortcuts={isMac ? 'Meta+K' : 'Control+K'}
        >
          <Icon name="search" size={16} />
          <span>{t('search.open')}</span>
          <kbd>{isMac ? 'Cmd K' : 'Ctrl K'}</kbd>
        </button>
      </TitleBar>
      <section class="content">
        {#key `${nav.view}:${nav.param ?? ''}`}
          <div class="view">
            {#if libraryView(nav.view) && library.trackCount === null && library.error}
              <div class="placeholder">
                <p>{t('library.loadFailed', { message: library.error })}</p>
              </div>
            {:else if libraryView(nav.view) && library.trackCount === null}
              <!-- The first load takes a moment; nothing flashes meanwhile. -->
            {:else if libraryView(nav.view) && library.trackCount === 0}
              <EmptyLibrary />
            {:else if nav.view === 'home' || nav.view === 'albums'}
              <AlbumGrid albums={library.albums ?? []} scrollKey="albums" header={albumsHead} />
            {:else if nav.view === 'album' && nav.param}
              <AlbumView albumId={nav.param} />
            {:else if nav.view === 'artists'}
              <ArtistList artists={library.artists ?? []} />
            {:else if nav.view === 'artist' && nav.param}
              <ArtistView name={nav.param} />
            {:else if nav.view === 'songs'}
              <SongList />
            {:else if nav.view === 'settings'}
              <Settings />
            {:else if nav.view === 'favorites'}
              <FavoritesView />
            {:else}
              <!-- Playlists arrive in a later version. -->
              <div class="placeholder">
                <h1>{t(titles[nav.view])}</h1>
                <p>{t(`view.${nav.view}.lead`)}</p>
              </div>
            {/if}
          </div>
        {/key}
      </section>
    </main>

    <div class="transport">
      <TransportStrip ringGap={RING + 44} />
    </div>

    {#if dragging}
      <div class="drop" aria-hidden="true">
        <div class="drop-card">
          <Icon name="drop" size={30} />
          <strong>{t('drop.hint')}</strong>
          <span>{t('drop.lead')}</span>
        </div>
      </div>
    {/if}
  </div>
</div>

<SearchPalette bind:open={searching} />

{#if flowMenu}
  <FlowMenu x={flowMenu.x} y={flowMenu.y} onclose={() => (flowMenu = null)} />
{/if}

{#if overlays.about}
  <About onclose={() => (overlays.about = false)} />
{/if}

<style>
  :global(html),
  :global(body) {
    height: 100%;
    overflow: hidden;
  }

  /* The window is transparent: only the app card and the Flow ring are painted. */
  :global(html),
  :global(body) {
    background: transparent;
  }

  /* The app card fills the window's width and top; there is no frame around it, so it never
     reads as a window inside a window. Below the card a transparent strip lets the Flow ring
     hang half out of the card's bottom edge; it is half a ring high plus room for the ring's
     bezel and glow, so nothing is cut off at the window edge. */
  .window {
    --ring-room: 48px;
    --frame-bottom: calc(var(--ring-size) / 2 + var(--ring-room));
    position: fixed;
    inset: 0;
    padding-bottom: var(--frame-bottom);
    background: transparent;
    user-select: none;
    -webkit-user-select: none;
  }

  .card {
    position: relative;
    height: 100%;
    display: grid;
    grid-template-columns: 220px minmax(0, 1fr);
    grid-template-rows: minmax(0, 1fr) auto;
    /* The window has no OS frame or shadow (that would paint the transparent strip), so the
       card draws its own rounded corners and edge. */
    border-radius: var(--radius-l);
    background: var(--surface);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.07);
    overflow: hidden;
  }

  .glass .card {
    background: rgba(20, 20, 24, 0.86);
  }

  .main {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    border-left: 1px solid var(--line);
    background:
      radial-gradient(60% 50% at 100% 0%, rgba(77, 166, 255, 0.05), transparent 70%),
      radial-gradient(40% 40% at 78% 0%, rgba(77, 255, 178, 0.03), transparent 70%),
      rgba(255, 255, 255, 0.012);
  }

  /* Each view scrolls itself: the long lists are virtualized against their own scroller. */
  .content {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .view {
    height: 100%;
    animation: fade var(--dur) ease-out;
  }

  .placeholder {
    max-width: 560px;
    padding: 4px 32px 32px;
  }

  h1 {
    margin: 6px 0 10px;
    font-family: var(--font-display);
    font-size: 32px;
    font-weight: 600;
    letter-spacing: -0.02em;
    line-height: 1.1;
  }

  p {
    margin: 0;
    font-size: 14px;
    line-height: 1.55;
    color: var(--text-dim);
  }

  .search {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 300px;
    max-width: 100%;
    height: 34px;
    padding: 0 7px 0 12px;
    border: 0;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.055);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
    color: rgba(243, 243, 245, 0.5);
    font: inherit;
    font-size: 13px;
    cursor: text;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .search:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text-dim);
  }

  .search:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
  }

  .search span {
    flex: 1;
    text-align: left;
  }

  .search kbd {
    padding: 2px 6px;
    border-radius: 5px;
    font: inherit;
    font-size: 11px;
    color: rgba(243, 243, 245, 0.5);
    background: rgba(255, 255, 255, 0.06);
  }

  .transport {
    grid-column: 1 / -1;
  }

  .drop {
    position: absolute;
    inset: 0;
    z-index: 5;
    display: grid;
    place-items: center;
    background: rgba(13, 13, 16, 0.72);
    backdrop-filter: blur(6px);
    animation: fade var(--dur) ease-out;
  }

  .drop-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 34px 48px;
    border-radius: 20px;
    border: 1.5px dashed rgba(243, 243, 245, 0.28);
    text-align: center;
  }

  .drop-card strong {
    margin-top: 6px;
    font-family: var(--font-display);
    font-size: 20px;
    font-weight: 600;
  }

  .drop-card span {
    max-width: 280px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-dim);
  }

  .ring-dock {
    position: absolute;
    left: 50%;
    /* Centred on the card's bottom edge: the upper half sits in the transport strip, the
       lower half hangs out of the app into the transparent strip below. */
    bottom: var(--ring-room);
    width: var(--ring);
    height: var(--ring);
    transform: translateX(-50%);
    z-index: 3;
    border-radius: 50%;
    /* A dark bezel, so the ring reads as set into the card's edge. */
    box-shadow: 0 0 0 6px var(--bg);
    background: var(--bg);
  }

  /* Always in the DOM as a live region; only its text changes. It floats above the seek
     rail (the strip is 84 px, the ring's upper half 42 px of it). */
  .notice {
    position: absolute;
    bottom: calc(100% + 42px + 14px);
    left: 50%;
    transform: translate(-50%, 4px);
    pointer-events: none;
    opacity: 0;
    transition:
      opacity var(--dur) ease-out,
      transform var(--dur) ease-out;
    width: max-content;
    max-width: 420px;
    padding: 8px 12px;
    border-radius: 10px;
    background: var(--raised);
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.08),
      0 10px 30px rgba(0, 0, 0, 0.5);
    font-size: 12.5px;
  }

  .notice.shown {
    opacity: 1;
    transform: translate(-50%, 0);
  }

  .notice:empty {
    padding: 0;
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }
</style>
