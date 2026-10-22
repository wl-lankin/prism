<script lang="ts">
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { untrack } from 'svelte';
  import About from './lib/components/About.svelte';
  import AlbumGrid from './lib/components/AlbumGrid.svelte';
  import HomeStations from './lib/components/HomeStations.svelte';
  import AlbumView from './lib/components/AlbumView.svelte';
  import ArtistList from './lib/components/ArtistList.svelte';
  import ArtistView from './lib/components/ArtistView.svelte';
  import EmptyLibrary from './lib/components/EmptyLibrary.svelte';
  import FavoritesView from './lib/components/FavoritesView.svelte';
  import FlowMenu from './lib/components/FlowMenu.svelte';
  import FlowRing, { type RingMode } from './lib/components/FlowRing.svelte';
  import Icon from './lib/components/Icon.svelte';
  import NowPlaying from './lib/components/NowPlaying.svelte';
  import RadioView from './lib/components/RadioView.svelte';
  import YouTubeView from './lib/components/YouTubeView.svelte';
  import PlaylistsView from './lib/components/PlaylistsView.svelte';
  import PlaylistView from './lib/components/PlaylistView.svelte';
  import QueuePanel from './lib/components/QueuePanel.svelte';
  import { loadQueuePanel, toggleQueuePanel } from './lib/stores/queue.svelte';
  import { initPlaylists } from './lib/stores/playlists.svelte';
  import SearchPalette from './lib/components/SearchPalette.svelte';
  import Settings from './lib/components/Settings.svelte';
  import Sidebar from './lib/components/Sidebar.svelte';
  import ResizeBand from './lib/components/ResizeBand.svelte';
  import Segmented from './lib/components/Segmented.svelte';
  import SongList from './lib/components/SongList.svelte';
  import SourceEmpty from './lib/components/SourceEmpty.svelte';
  import TitleBar from './lib/components/TitleBar.svelte';
  import TransportStrip from './lib/components/TransportStrip.svelte';
  import ViewHead from './lib/components/ViewHead.svelte';
  import { activateIdle, holdToOpen } from './lib/flow';
  import { lang, t, tc, type Key } from './lib/i18n';
  import { onNavGo, playerPlayFiles, playerToggle, startupNotice, traySetLabels, windowVibrancy } from './lib/ipc';
  import { favorites, initFavorites } from './lib/stores/favorites.svelte';
  import { albumsOf, artistsOf, countOf, hasMixedSources, initLibrary, library } from './lib/stores/library.svelte';
  import { failureText, initNavidrome, navidrome } from './lib/stores/navidrome.svelte';
  import { initRadio, radio } from './lib/stores/radio.svelte';
  import { initYtm, ytm } from './lib/stores/ytm.svelte';
  import { closeOverlay, openOverlay } from './lib/stores/overlays.svelte';
  import { ytmIdle } from './lib/format';
  import { go, nav, overlays, type View } from './lib/stores/nav.svelte';
  import { notice, notify } from './lib/stores/notice.svelte';
  import { followPlayerErrors, initPlayer, player } from './lib/stores/player.svelte';
  import { initRing, ring } from './lib/stores/ring.svelte';
  import { followSettings, initSettings } from './lib/stores/settings.svelte';
  import { initWindow, windowState } from './lib/stores/window.svelte';
  import shape from './lib/window-shape.json';

  /** The window's shape, shared with the backend's click-through region. The Flow ring hangs
   *  half out below the card's bottom edge, into the transparent strip at the bottom of the
   *  window. A maximized window has no strip: the ring, smaller, sits inside the transport. */
  const MAIN = shape.main;
  /** The transport strip's height (`TransportStrip`). */
  const TRANSPORT = 84;
  /** The notice floats this far above the card's bottom edge, clear of the seek rail. */
  const NOTICE_LIFT = TRANSPORT + 20;
  /** The radius of the window shape's circle around the ring (`shell::shape::main_region`):
   *  the ring's glow ends inside it. A maximized window has no shape. */
  const RING_REACH = MAIN.ring / 2 + MAIN.bezel + MAIN.glow;

  const maximized = $derived(windowState.maximized);
  const ringSize = $derived(maximized ? MAIN.ringMaximized : MAIN.ring);
  const strip = $derived(maximized ? 0 : MAIN.strip);
  /** The ring dock's bottom, measured from the window's bottom edge. */
  const dockBottom = $derived(maximized ? (TRANSPORT - ringSize) / 2 : MAIN.ringRoom + MAIN.ringLift);
  /** The dock's top above the card's bottom edge, for the notice above it. */
  const dockTop = $derived(maximized ? dockBottom + ringSize : ringSize / 2 + MAIN.ringLift);
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

  $effect(() => initRing());

  // The queue panel opens as it was left.
  $effect(() => loadQueuePanel());

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
    let stop: (() => void) | undefined;
    let gone = false;
    initPlaylists()
      .then((cleanup) => (gone ? cleanup() : (stop = cleanup)))
      .catch((err) => console.error('loading the playlists failed', err));
    return () => {
      gone = true;
      stop?.();
    };
  });

  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    initRadio()
      .then((cleanup) => (gone ? cleanup() : (stop = cleanup)))
      .catch((err) => console.error('loading the radio state failed', err));
    return () => {
      gone = true;
      stop?.();
    };
  });

  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    initYtm()
      .then((cleanup) => (gone ? cleanup() : (stop = cleanup)))
      .catch((err) => console.error('loading the YouTube Music state failed', err));
    return () => {
      gone = true;
      stop?.();
    };
  });

  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    initNavidrome()
      .then((cleanup) => (gone ? cleanup() : (stop = cleanup)))
      .catch((err) => console.error('loading the Navidrome status failed', err));
    return () => {
      gone = true;
      stop?.();
    };
  });

  // One notice when a Navidrome track pauses because the server cannot be reached.
  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    followPlayerErrors()
      .then((unlisten) => (gone ? unlisten() : (stop = unlisten)))
      .catch((err) => console.error('following the player errors failed', err));
    return () => {
      gone = true;
      stop?.();
    };
  });

  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    try {
      initWindow()
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

  // The mini player sends the main window places to go (its cover opens Now Playing).
  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    onNavGo((target) => go(target.view, target.param))
      .then((unlisten) => (gone ? unlisten() : (stop = unlisten)))
      .catch(() => {});
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

  // Saves from the mini player (or any other window) show up here too.
  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    followSettings()
      .then((unlisten) => (gone ? unlisten() : (stop = unlisten)))
      .catch(() => {});
    return () => {
      gone = true;
      stop?.();
    };
  });

  // The tray menu speaks the UI language: reading `t()` here re-sends it on every change.
  $effect(() => {
    traySetLabels({
      open: t('tray.open'),
      playPause: t('tray.playPause'),
      next: t('tray.next'),
      previous: t('tray.previous'),
      quit: t('tray.quit'),
    }).catch(() => {});
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
    // Ctrl+Q (also on macOS, where Cmd+Q quits) opens and closes "Up next".
    if (event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey && event.key.toLowerCase() === 'q') {
      event.preventDefault();
      toggleQueuePanel();
    }
  }

  /** YouTube Music answers the transport while it is the active player. */
  const yt = $derived(player.active === 'ytm');
  const ytIdle = $derived(ytmIdle(ytm.state));
  const mode: RingMode = $derived(
    yt
      ? ytIdle
        ? 'flow'
        : ytm.state.status === 'playing'
          ? 'pause'
          : 'play'
      : player.status === 'idle'
        ? 'flow'
        : player.status === 'playing'
          ? 'pause'
          : 'play',
  );
  const progress = $derived(
    yt
      ? ytm.state.durationMs > 0
        ? ytm.positionMs / ytm.state.durationMs
        : 0
      : player.durationMs > 0
        ? player.positionMs / player.durationMs
        : 0,
  );

  // The notice and the drop overlay sit inside the content area, above the YouTube Music
  // view, so each hides it while it shows. The registry reads the set it writes: untrack.
  // Only whether a notice shows is tracked: a replaced text must not close and reopen it.
  const noticeShown = $derived(!!notice.text);
  $effect(() => {
    if (!noticeShown) return;
    untrack(() => openOverlay('notice'));
    return () => untrack(() => closeOverlay('notice'));
  });
  $effect(() => {
    if (!dragging) return;
    untrack(() => openOverlay('drop'));
    return () => untrack(() => closeOverlay('drop'));
  });

  async function activate() {
    if (yt ? !ytIdle : player.status !== 'idle') {
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
    if (!box) return;
    // The menu hands focus back to whatever had it when it opened: make that the ring.
    dock?.querySelector<HTMLElement>('.ring')?.focus();
    flowMenu = { x: box.left + box.width / 2, y: box.top };
  }

  /** Views that show the library, and so the empty state when it has no tracks. */
  function libraryView(view: View): boolean {
    return view === 'home' || view === 'albums' || view === 'artists' || view === 'songs';
  }

  // Source views: albums, artists and songs narrowed to `nav.source`. The lists are narrowed
  // once per view (and per library reload), not on every render.
  type SourceTab = 'albums' | 'artists' | 'songs';
  const shownAlbums = $derived(albumsOf(nav.source));
  const shownArtists = $derived(artistsOf(nav.source));
  /** Tracks of the shown source (or of every source); null before the first load. */
  const shownTracks = $derived(
    library.trackCounts ? (nav.source ? countOf(library.trackCounts, nav.source) : library.trackCounts.all) : null,
  );
  /** The views of every source mark their items with a source, once there is more than one. */
  const mixed = $derived(!nav.source && hasMixedSources());
  /** An empty view of every source while Navidrome is signed in says what Navidrome is doing
   *  (the first sync, mostly) rather than asking for a music folder. */
  const emptyAsNavidrome = $derived(nav.source === 'navidrome' || (!nav.source && navidrome.configured));
  const hasHomeStations = $derived(radio.saved.favorites.some((x) => favorites.stations.has(x.id)));

  const sourceTabs = $derived<{ value: SourceTab; label: string }[]>([
    { value: 'albums', label: t('nav.albums') },
    { value: 'artists', label: t('nav.artists') },
    { value: 'songs', label: t('nav.songs') },
  ]);

  /** A calm chip beside a Navidrome view's switch: the index stays browsable meanwhile. The
   *  visible label is short (the header must fit at the minimum window width); the full
   *  sentence is its tooltip and accessible name. */
  const sourceHint = $derived.by(() => {
    if (nav.source !== 'navidrome') return null;
    if (navidrome.state === 'offline') {
      return { text: t('sourceView.offlineShort'), full: t('sourceView.offline'), away: true };
    }
    if (navidrome.state === 'error') {
      const sentence = t('sourceView.failed');
      const full = navidrome.error ? `${sentence}. ${failureText(navidrome.error)}` : sentence;
      return { text: t('sourceView.failedShort'), full, away: true };
    }
    const progress = navidrome.progress;
    if (navidrome.state === 'syncing' && progress) {
      const fmt = (n: number) => n.toLocaleString(lang());
      if (progress.total) {
        const percent = Math.min(100, Math.floor((progress.done / progress.total) * 100));
        const full = t('sourceView.syncing', { done: fmt(progress.done), total: fmt(progress.total) });
        return { text: t('sourceView.syncingShort', { percent }), full, away: false };
      }
      const full = t('sourceView.syncingCount', { done: fmt(progress.done) });
      return { text: t('sourceView.syncingBusy'), full, away: false };
    }
    return null;
  });

  /** What screen readers hear about the Navidrome link: its state changes (offline, failed,
   *  a sync starting or finishing), never every progress number. */
  let linkAnnouncement = $state('');
  let lastLinkState: string | null = null;
  $effect(() => {
    const state = navidrome.state;
    const previous = lastLinkState;
    lastLinkState = state;
    if (previous === null || previous === state) return;
    if (state === 'offline') linkAnnouncement = t('sourceView.offline');
    else if (state === 'error') linkAnnouncement = t('sourceView.failed');
    else if (state === 'syncing') linkAnnouncement = t('sourceView.syncStarted');
    else if (state === 'online' && previous === 'syncing') linkAnnouncement = t('sourceView.syncDone');
    else linkAnnouncement = '';
  });

  /** Views tinted by a cover backdrop: the title bar floats over them, so the tint runs up to
   *  the card's top edge instead of stopping under a plain header band. */
  const immersive = $derived(nav.view === 'album' || nav.view === 'playlist' || nav.view === 'nowplaying');

  const titles: Record<View, Key> = {
    home: 'nav.home',
    favorites: 'nav.favorites',
    playlists: 'nav.playlists',
    playlist: 'nav.playlists',
    albums: 'nav.albums',
    artists: 'nav.artists',
    songs: 'nav.songs',
    album: 'nav.albums',
    artist: 'nav.artists',
    nowplaying: 'nowPlaying.title',
    radio: 'radio.title',
    youtube: 'ytm.title',
    settings: 'nav.settings',
  };
</script>

<svelte:window {onkeydown} />

{#snippet albumsHead()}
  {#if nav.source}
    {@render sourceHead(tc('view.count.albums', shownAlbums.length))}
  {:else}
    <ViewHead title={t('nav.albums')} meta={tc('view.count.albums', shownAlbums.length)} />
    {#if nav.view === 'home'}<HomeStations />{/if}
  {/if}
{/snippet}

{#snippet artistsHead()}
  {@render sourceHead(tc('view.count.artists', shownArtists.length))}
{/snippet}

{#snippet songsHead()}
  {@render sourceHead(tc('view.count.songs', shownTracks ?? 0))}
{/snippet}

<!-- A source view: the source's name, its count, and a switch between its albums, artists
     and songs. -->
{#snippet sourceHead(meta: string)}
  <ViewHead title={nav.source ? t(`source.${nav.source}`) : ''} {meta} actions={sourceActions} />
{/snippet}

{#snippet sourceActions()}
  {#if sourceHint?.away && navidrome.state === 'error'}
    <button type="button" class="source-hint away" title={sourceHint.full} aria-label={sourceHint.full} onclick={() => go('settings')}>
      <i aria-hidden="true"></i><span class="hint-text">{sourceHint.text}</span>
    </button>
  {:else if sourceHint}
    <span class="source-hint" class:away={sourceHint.away} title={sourceHint.full} role="note" aria-label={sourceHint.full}>
      <i aria-hidden="true"></i><span class="hint-text">{sourceHint.text}</span>
    </span>
  {/if}
  <span id="source-switch-label" class="sr-only">{t('sourceView.show')}</span>
  <Segmented
    options={sourceTabs}
    value={nav.view as SourceTab}
    labelledby="source-switch-label"
    attr="data-view"
    onpick={(view) => go(view, undefined, nav.source)}
  />
{/snippet}

<div
  class="window"
  class:glass
  class:maximized
  data-tauri-drag-region
  style="--strip:{strip}px;--corner:{maximized ? 0 : MAIN.corner}px"
>
  <!-- First in the DOM so the main control comes early in the tab order; placed by CSS. -->
  <div
    class="ring-dock"
    style="--ring:{ringSize}px;--bezel:{MAIN.bezel}px;--dock-bottom:{dockBottom}px;--notice-lift:{NOTICE_LIFT - dockTop}px"
    bind:this={dock}
    use:holdToOpen={openFlowMenu}
  >
    <div class="notice" class:shown={notice.text} role="status" aria-live="polite">{notice.text ?? ''}</div>
    <FlowRing
      {mode}
      {progress}
      colors={ring.colors}
      level={ring.level}
      size={ringSize}
      reach={maximized ? undefined : RING_REACH}
      onactivate={activate}
      oncontextmenu={openFlowMenu}
    />
  </div>

  <div class="card">
    <Sidebar />

    <main class="main" class:immersive>
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
      <!-- Outside the keyed view, so a state change is heard once, whichever view is open. -->
      <span class="sr-only" role="status" aria-live="polite">{linkAnnouncement}</span>
      <!-- data-menu-bounds: context menus stay inside it, clear of the transport and the ring. -->
      <section class="content" data-menu-bounds>
        {#key `${nav.view}:${nav.param ?? ''}:${nav.source ?? ''}`}
          <div class="view">
            {#if libraryView(nav.view) && library.trackCounts === null && library.error}
              <div class="placeholder">
                <p>{t('library.loadFailed', { message: library.error })}</p>
              </div>
            {:else if libraryView(nav.view) && library.trackCounts === null}
              <!-- The first load takes a moment; nothing flashes meanwhile. -->
            {:else if libraryView(nav.view) && shownTracks === 0}
              <!-- Under favorite stations, the empty library is a quiet section of Home
                   rather than the big centred invitation. -->
              {@const withStations = nav.view === 'home' && !nav.source && hasHomeStations}
              <div class="empty-home" class:with-stations={withStations}>
                {#if nav.view === 'home' && !nav.source}
                  <div class="home-stations"><HomeStations /></div>
                {/if}
                <div class="empty-rest">
                  {#if emptyAsNavidrome}
                    <SourceEmpty source="navidrome" offerLocal={!nav.source} />
                  {:else}
                    <EmptyLibrary compact={withStations} />
                  {/if}
                </div>
              </div>
            {:else if nav.view === 'home' || nav.view === 'albums'}
              <AlbumGrid albums={shownAlbums} scrollKey="albums:{nav.source ?? 'all'}" header={albumsHead} showSource={mixed} />
            {:else if nav.view === 'album' && nav.param}
              <AlbumView albumId={nav.param} />
            {:else if nav.view === 'artists'}
              <ArtistList artists={shownArtists} showSource={mixed} header={nav.source ? artistsHead : undefined} />
            {:else if nav.view === 'artist' && nav.param}
              <ArtistView name={nav.param} />
            {:else if nav.view === 'songs'}
              <SongList source={nav.source} showSource={mixed} header={nav.source ? songsHead : undefined} />
            {:else if nav.view === 'settings'}
              <Settings />
            {:else if nav.view === 'favorites'}
              <FavoritesView />
            {:else if nav.view === 'radio'}
              <RadioView />
            {:else if nav.view === 'youtube'}
              <YouTubeView />
            {:else if nav.view === 'nowplaying'}
              <NowPlaying />
            {:else if nav.view === 'playlists'}
              <PlaylistsView />
            {:else if nav.view === 'playlist' && nav.param}
              <PlaylistView playlistId={nav.param.startsWith('navidrome:') ? nav.param : Number(nav.param)} />
            {:else}
              <div class="placeholder">
                <h1>{t(titles[nav.view])}</h1>
                <p>{t(`view.${nav.view}.lead`)}</p>
              </div>
            {/if}
          </div>
        {/key}
      </section>
      <QueuePanel />
    </main>

    <div class="transport">
      <TransportStrip ringGap={ringSize + 44} ringInside={maximized} />
    </div>

    <!-- The strip below the card is click-through, so the card's bottom edge resizes. -->
    <ResizeBand gap={ringSize + 2 * MAIN.bezel} />

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
  /* `clip`, not `hidden`: a hidden overflow can still be scrolled by script (a focus or
     scrollIntoView that reaches past the edge), which shifted the whole card left by the
     sidebar's width for a moment when a view opened (owner, 2026-10-22). */
  :global(html),
  :global(body) {
    height: 100%;
    overflow: clip;
  }

  /* The window is transparent: only the app card and the Flow ring are painted. */
  :global(html),
  :global(body) {
    background: transparent;
  }

  /* The app card fills the window's width and top; there is no frame around it, so it never
     reads as a window inside a window. Below the card a transparent strip (`--strip`, from
     window-shape.json) lets the Flow ring hang half out of the card's bottom edge; it is half
     a ring high plus room for the ring's bezel and glow, so nothing is cut off at the window
     edge. The backend makes the strip click-through beside the ring. A maximized window has
     no strip and square corners. */
  .window {
    position: fixed;
    inset: 0;
    padding-bottom: var(--strip);
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
    border-radius: var(--corner);
    background: var(--surface);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.07);
    /* Never scrollable, not even by script (see html, body). */
    overflow: clip;
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
    overflow: clip;
  }

  /* Over a backdrop view the title bar floats: the view starts at the card's top edge, so its
     backdrop runs behind the bar, and the view keeps its content clear of the bar with
     `--view-top`. The bar stays put while the view scrolls. A view that scrolls names its
     scroller `--view-scroll`; scrolled content then fades in a blurred scrim behind the bar,
     so the search field and the window controls stay readable over it. At the top there is
     no scrim, and the backdrop shows through untouched. */
  .main.immersive {
    timeline-scope: --view-scroll;
  }

  .main.immersive > :global(.titlebar) {
    position: absolute;
    inset: 0 0 auto 0;
    z-index: 2;
  }

  .main.immersive > :global(.titlebar)::before {
    content: '';
    position: absolute;
    inset: 0 0 -18px 0;
    z-index: -1;
    pointer-events: none;
    background: rgba(16, 16, 20, 0.42);
    backdrop-filter: blur(18px) saturate(1.2);
    mask-image: linear-gradient(to bottom, #000 calc(100% - 26px), transparent);
    opacity: 0;
    animation: scrim linear both;
    animation-timeline: --view-scroll;
    animation-range: 0 96px;
  }

  @keyframes scrim {
    to {
      opacity: 1;
    }
  }

  /* The backdrop keeps most of its colour up to the edge: there is no header line to fade into. */
  .main.immersive .content {
    --view-top: var(--header-h);
    --top-alpha: 0.8;
  }

  .view {
    height: 100%;
    animation: fade var(--dur) ease-out;
  }

  .empty-home {
    height: 100%;
    display: flex;
    flex-direction: column;
  }

  .home-stations {
    flex: none;
    padding: 10px 32px 0;
  }

  .empty-rest {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
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

  /* A quiet status beside a source view's switch: a small dot in the source color (hollow
     while the server is away) and one line of dim text. */
  .source-hint {
    display: inline-flex;
    align-self: center;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 11px 0 10px;
    border: 0;
    border-radius: 14px;
    background: rgba(255, 255, 255, 0.04);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
    color: var(--text-dim);
    font: inherit;
    font-size: 12.5px;
    white-space: nowrap;
    animation: fade var(--dur) ease-out;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  /* At the minimum window width the chip is what gives way (its label ellipsizes); the
     switch and the title keep their size. */
  .source-hint {
    min-width: 0;
    flex-shrink: 1;
  }

  .content :global(.viewhead .actions) {
    min-width: 0;
  }

  .content :global(.viewhead .segments) {
    flex: none;
  }

  .hint-text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .source-hint i {
    width: 7px;
    height: 7px;
    flex: none;
    border-radius: 50%;
    background: var(--src-navidrome);
    box-shadow: 0 0 8px color-mix(in srgb, var(--src-navidrome) 45%, transparent);
  }

  .source-hint.away i {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--src-navidrome);
  }

  button.source-hint {
    cursor: pointer;
  }

  button.source-hint:hover {
    background: rgba(255, 255, 255, 0.07);
    color: var(--text);
  }

  button.source-hint:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
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
       lower half hangs out of the app into the transparent strip below. Maximized, it is
       centred in the transport strip instead. */
    bottom: var(--dock-bottom);
    width: var(--ring);
    height: var(--ring);
    transform: translateX(-50%);
    z-index: 3;
    border-radius: 50%;
    /* A dark bezel, so the ring reads as set into the card's edge. */
    box-shadow: 0 0 0 var(--bezel) var(--bg);
    background: var(--bg);
  }

  /* Always in the DOM as a live region; only its text changes. It floats above the seek
     rail, at the same height whether the ring hangs out or sits inside the strip. */
  .notice {
    position: absolute;
    bottom: calc(100% + var(--notice-lift));
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
