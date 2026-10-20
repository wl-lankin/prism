<script lang="ts">
  import FlowRing, { type RingMode } from './lib/components/FlowRing.svelte';
  import Sidebar from './lib/components/Sidebar.svelte';
  import TitleBar from './lib/components/TitleBar.svelte';
  import TransportStrip from './lib/components/TransportStrip.svelte';
  import { playAllShuffled } from './lib/flow';
  import { t, type Key } from './lib/i18n';
  import { playerToggle, windowVibrancy } from './lib/ipc';
  import { go, nav, type View } from './lib/stores/nav.svelte';
  import { initPlayer, player } from './lib/stores/player.svelte';

  const RING = 84;

  let glass = $state(false);
  let starting = $state(false);
  let notice = $state<string | null>(null);
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;

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

  const mode: RingMode = $derived(
    player.status === 'idle' ? 'flow' : player.status === 'playing' ? 'pause' : 'play',
  );
  const progress = $derived(player.durationMs > 0 ? player.positionMs / player.durationMs : 0);

  function show(message: string) {
    notice = message;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = null), 5000);
  }

  async function activate() {
    if (player.status !== 'idle') {
      playerToggle().catch((err) => show(t('flow.failed', { message: String(err) })));
      return;
    }
    if (starting) return;
    starting = true;
    try {
      // Flow proper arrives in M2; until then an idle click plays the whole library shuffled.
      if ((await playAllShuffled()) === 'empty') go('settings');
    } catch (err) {
      show(t('flow.failed', { message: String(err) }));
    } finally {
      starting = false;
    }
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

<div class="window" class:glass data-tauri-drag-region>
  <!-- First in the DOM so the main control comes early in the tab order; placed by CSS. -->
  <div class="ring-dock" style="--ring:{RING}px">
    <div class="notice" class:shown={notice} role="status" aria-live="polite">{notice ?? ''}</div>
    <FlowRing {mode} {progress} size={RING} onactivate={activate} />
  </div>

  <div class="card">
    <Sidebar />

    <main class="main">
      <TitleBar />
      <section class="content">
        <!-- Task 11 fills the library views, Task 12 Settings; until then a quiet placeholder. -->
        {#key nav.view}
          <div class="placeholder">
            <h1>{t(titles[nav.view])}</h1>
            <p>{t(`view.${nav.view}.lead`)}</p>
          </div>
        {/key}
      </section>
    </main>

    <div class="transport">
      <TransportStrip ringGap={RING + 44} />
    </div>
  </div>
</div>

<style>
  :global(html),
  :global(body) {
    height: 100%;
    overflow: hidden;
  }

  /* On a glass material the page itself turns see-through; without one it stays opaque. */
  :global(html[data-vibrancy='on']),
  :global(html[data-vibrancy='on'] body) {
    background: transparent;
  }

  .window {
    --frame-bottom: 52px;
    position: fixed;
    inset: 0;
    padding: 16px 16px var(--frame-bottom);
    background: var(--bg);
    user-select: none;
    -webkit-user-select: none;
  }

  .window.glass {
    background: rgba(13, 13, 16, 0.38);
  }

  .card {
    position: relative;
    height: 100%;
    display: grid;
    grid-template-columns: 220px minmax(0, 1fr);
    grid-template-rows: minmax(0, 1fr) auto;
    border-radius: var(--radius-l);
    background: var(--surface);
    box-shadow:
      inset 0 0 0 1px rgba(255, 255, 255, 0.06),
      0 1px 0 rgba(255, 255, 255, 0.03),
      0 24px 60px -18px rgba(0, 0, 0, 0.75),
      0 8px 24px rgba(0, 0, 0, 0.35);
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

  .content {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 4px 32px 32px;
  }

  .placeholder {
    max-width: 560px;
    animation: fade var(--dur) ease-out;
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

  .transport {
    grid-column: 1 / -1;
  }

  .ring-dock {
    position: absolute;
    left: 50%;
    bottom: calc(var(--frame-bottom) - var(--ring) / 2);
    width: var(--ring);
    height: var(--ring);
    transform: translateX(-50%);
    z-index: 2;
    border-radius: 50%;
    /* A notch of the frame around the ring, so it reads as set into the card's edge. */
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
