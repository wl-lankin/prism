<script lang="ts">
  import { sourceQualityLabel } from '../format';
  import { holdToOpen } from '../flow';
  import { t } from '../i18n';
  import {
    flowStart,
    mainShow,
    miniLayout,
    miniMenu,
    navGoMain,
    onMiniLayout,
    onMiniShown,
    playerNext,
    playerPrev,
    playerSetVolume,
    playerToggle,
  } from '../ipc';
  import { initFavorites } from '../stores/favorites.svelte';
  import { notice, notify } from '../stores/notice.svelte';
  import { initPlayer, player } from '../stores/player.svelte';
  import { initRing, ring } from '../stores/ring.svelte';
  import { followSettings, initSettings } from '../stores/settings.svelte';
  import type { FlowVariant } from '../types';
  import shape from '../window-shape.json';
  import Cover from './Cover.svelte';
  import FlowMenu from './FlowMenu.svelte';
  import FlowRing, { type RingMode } from './FlowRing.svelte';
  import Heart from './Heart.svelte';
  import Icon from './Icon.svelte';
  import Slider from './Slider.svelte';

  /** The mini player, the whole page of the `mini` window. A card with the Flow ring hanging
   *  half out of its left edge, the ring's light washing into the card. The window never
   *  takes focus, so everything here works with the pointer alone. The window is taller than
   *  the player: the Flow menu opens into the room above it (below it under a menu bar). */
  const MINI = shape.mini;
  const COVER = 46;

  let top = $state(MINI.menuRoom);
  let flowMenu: { x: number; y: number } | null = $state(null);
  let starting = $state(false);
  let dock: HTMLDivElement | undefined = $state();

  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    initPlayer()
      .then((unlisten) => (gone ? unlisten() : (stop = unlisten)))
      .catch((err) => console.error('connecting to the player failed', err));
    return () => {
      gone = true;
      stop?.();
    };
  });

  $effect(() => initRing());

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

  const loadSettings = () => initSettings().catch((err) => console.error('loading the settings failed', err));

  // The language is a setting: saves from the main window arrive as `settings://changed`.
  // The window is shown without activation, so the page sees no focus or visibility change:
  // the backend says `mini://shown` instead. Then the settings are read again (in case an
  // event was missed) and a Flow menu left open from last time closes.
  $effect(() => {
    loadSettings();
    const stops: (() => void)[] = [];
    let gone = false;
    const keep = (stop: () => void) => (gone ? stop() : stops.push(stop));
    followSettings()
      .then(keep)
      .catch(() => {});
    onMiniShown(() => {
      closeFlowMenu();
      loadSettings();
    })
      .then(keep)
      .catch(() => {});
    return () => {
      gone = true;
      for (const stop of stops) stop();
    };
  });

  $effect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    miniLayout()
      .then((layout) => {
        if (layout) top = layout.top;
      })
      .catch(() => {});
    onMiniLayout((layout) => {
      flowMenu = null;
      top = layout.top;
    })
      .then((unlisten) => (gone ? unlisten() : (stop = unlisten)))
      .catch(() => {});
    return () => {
      gone = true;
      stop?.();
    };
  });

  const track = $derived(player.track);
  const idle = $derived(player.status === 'idle' || !track);
  const mode: RingMode = $derived(player.status === 'idle' ? 'flow' : player.status === 'playing' ? 'pause' : 'play');
  const progress = $derived(player.durationMs > 0 ? player.positionMs / player.durationMs : 0);
  const byline = $derived(track ? [track.artist, track.album].filter(Boolean).join(' · ') : '');
  const source = $derived(track ? sourceQualityLabel(track, (s) => t(`source.${s}`)) : '');

  let lastVolume = 0.8;
  const muted = $derived(player.volume <= 0.001);
  const volumeIcon = $derived(muted ? 'mute' : player.volume < 0.5 ? 'volumeLow' : 'volume');

  function report(what: string) {
    return (err: unknown) => console.error(`${what} failed`, err);
  }

  function setVolume(v: number) {
    player.volume = Math.min(Math.max(v, 0), 1);
    playerSetVolume(player.volume).catch(report('setting the volume'));
  }

  function toggleMute() {
    if (muted) setVolume(lastVolume > 0.001 ? lastVolume : 0.5);
    else {
      lastVolume = player.volume;
      setVolume(0);
    }
  }

  /** The wheel over the volume nudges it, 5 % a notch. */
  function onwheel(event: WheelEvent) {
    if (event.deltaY === 0) return;
    event.preventDefault();
    setVolume(Math.round((player.volume + (event.deltaY < 0 ? 0.05 : -0.05)) * 100) / 100);
  }

  /** A Flow from here: there is no library view to send an empty library to, so the mini
   *  player says so itself, like the fallback from favorites to the whole library. */
  async function startFlow(variant: FlowVariant) {
    if (starting) return;
    starting = true;
    try {
      const outcome = await flowStart(variant);
      if (outcome.outcome === 'empty') notify(t('mini.emptyLibrary'));
      else if (variant.kind === 'favorites' && outcome.variant.kind === 'library') notify(t('flow.noFavorites'));
    } catch (err) {
      notify(t('flow.failed', { message: String(err) }));
    } finally {
      starting = false;
    }
  }

  function activate() {
    if (player.status !== 'idle') {
      playerToggle().catch((err) => notify(t('flow.failed', { message: String(err) })));
      return;
    }
    void startFlow({ kind: 'favorites' });
  }

  async function openFlowMenu() {
    const box = dock?.getBoundingClientRect();
    if (!box || flowMenu) return;
    // The whole window paints and takes clicks while the menu is open.
    await miniMenu(true).catch(report('opening the Flow menu'));
    // Into the room: above the ring, or below it when the room is below the player.
    flowMenu = { x: box.left + box.width / 2, y: top > 0 ? box.top : box.bottom };
  }

  function closeFlowMenu() {
    if (!flowMenu) return;
    flowMenu = null;
    clearTimeout(leaving);
    miniMenu(false).catch(report('closing the Flow menu'));
  }

  /** The window never has focus, so a click in another app does not close the menu:
   *  leaving the window with the pointer does, after a moment to come back. */
  let leaving: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    if (!flowMenu) return;
    const leave = () => {
      clearTimeout(leaving);
      leaving = setTimeout(closeFlowMenu, 700);
    };
    const enter = () => clearTimeout(leaving);
    document.documentElement.addEventListener('mouseleave', leave);
    document.documentElement.addEventListener('mouseenter', enter);
    return () => {
      clearTimeout(leaving);
      document.documentElement.removeEventListener('mouseleave', leave);
      document.documentElement.removeEventListener('mouseenter', enter);
    };
  });

  function openPrism() {
    mainShow().catch(report('opening Prism'));
  }

  /** The cover opens Now Playing in the main window. */
  async function openNowPlaying() {
    await navGoMain({ view: 'nowplaying' }).catch(report('opening Now Playing'));
    openPrism();
  }
</script>

<div
  class="stage"
  style="top:{top}px;--pad-left:{MINI.padLeft}px;--pad-y:{MINI.padY}px;--w:{MINI.width}px;--h:{MINI.height}px;--ring:{MINI.ring}px;--bezel:{MINI.bezel}px;--corner:{MINI.corner}px;--c1:{ring.colors[0]};--c2:{ring.colors[1]};--c3:{ring.colors[2]};--c4:{ring.colors[3]};--lvl:{ring.level.toFixed(3)}"
>
  <section class="card" class:idle aria-label={t('mini.label')} oncontextmenu={(event) => event.preventDefault()}>
    <span class="wash" aria-hidden="true"></span>

    <div class="now" class:hearted={!idle && track && !track.id.startsWith('file:')}>
      {#if !idle && track}
        <button
          type="button"
          class="cover-button"
          aria-label={t('mini.openNowPlaying')}
          title={t('mini.openNowPlaying')}
          onclick={openNowPlaying}
        >
          <Cover coverId={track.coverId} size={COVER} alt="" />
        </button>
      {/if}
      <div class="meta" class:noticed={notice.text}>
        {#if idle}
          <div class="title">{t('mini.idleTitle')}</div>
          <div class="lines">
            <div class="sub wrap">{t('mini.idleHint')}</div>
          </div>
        {:else if track}
          <div class="title" title={track.title}>{track.title}</div>
          <div class="lines">
            <div class="sub" title={byline}>{player.status === 'loading' ? t('transport.loading') : byline}</div>
            <div class="source" style="--dot:var(--src-{track.source})" title={source}><span>{source}</span></div>
          </div>
        {/if}
        <p class="notice" role="status" aria-live="polite">{notice.text ?? ''}</p>
      </div>
      {#if !idle && track && !track.id.startsWith('file:')}
        <span class="heart">
          <Heart kind="track" id={track.id} size={16} label={t('favorites.toggleItem', { name: track.title })} />
        </span>
      {/if}
    </div>

    <div class="controls">
      <button
        type="button"
        class="btn"
        disabled={idle}
        aria-label={t('transport.previous')}
        title={t('transport.previous')}
        onclick={() => playerPrev().catch(report('previous'))}
      >
        <Icon name="prev" size={16} />
      </button>
      <button
        type="button"
        class="btn"
        disabled={idle}
        aria-label={t('transport.next')}
        title={t('transport.next')}
        onclick={() => playerNext().catch(report('next'))}
      >
        <Icon name="next" size={16} />
      </button>
      <div class="volume" {onwheel}>
        <button
          type="button"
          class="btn"
          aria-label={muted ? t('transport.unmute') : t('transport.mute')}
          title={muted ? t('transport.unmute') : t('transport.mute')}
          onclick={toggleMute}
        >
          <Icon name={volumeIcon} size={16} />
        </button>
        <div class="slider">
          <Slider
            value={player.volume}
            label={t('transport.volume')}
            valueText={`${Math.round(player.volume * 100)} %`}
            oninput={setVolume}
            oncommit={setVolume}
          />
        </div>
      </div>
      <button type="button" class="open" onclick={openPrism}>{t('mini.open')}</button>
    </div>
  </section>

  <div class="dock" bind:this={dock} use:holdToOpen={openFlowMenu}>
    <FlowRing
      {mode}
      {progress}
      colors={ring.colors}
      level={ring.level}
      size={MINI.ring}
      onactivate={activate}
      oncontextmenu={openFlowMenu}
    />
  </div>
</div>

{#if flowMenu}
  <FlowMenu x={flowMenu.x} y={flowMenu.y} filterable={false} onclose={closeFlowMenu} onpick={startFlow} />
{/if}

<style>
  /* The window is transparent; its shape (the card and the ring) is cut by the backend. */
  :global(html),
  :global(body) {
    height: 100%;
    overflow: hidden;
    background: transparent;
    user-select: none;
    -webkit-user-select: none;
  }

  .stage {
    position: fixed;
    left: 0;
    right: 0;
    height: calc(var(--h) + 2 * var(--pad-y));
    /* The cover colors fade like the ring's (the properties are registered by FlowRing). */
    transition:
      --c1 600ms ease,
      --c2 600ms ease,
      --c3 600ms ease,
      --c4 600ms ease;
  }

  .card {
    position: absolute;
    left: var(--pad-left);
    top: var(--pad-y);
    width: var(--w);
    height: var(--h);
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    /* Clear of the ring's right half and its bezel. */
    padding: 11px 12px 9px calc(var(--ring) / 2 + var(--bezel) + 13px);
    border-radius: var(--corner);
    overflow: hidden;
    isolation: isolate;
    background: linear-gradient(180deg, #1b1b21 0%, #151519 100%);
    box-shadow:
      inset 0 0 0 1px rgba(255, 255, 255, 0.075),
      inset 0 1px 0 rgba(255, 255, 255, 0.05);
    color: var(--text);
  }

  /* The ring's light on the card: the cover colors, strongest at the ring and fading to the
     right, rising a little with the music. */
  .wash {
    position: absolute;
    inset: 0;
    z-index: -1;
    pointer-events: none;
    background:
      radial-gradient(150px 110px at 0% 42%, color-mix(in srgb, var(--c1) 24%, transparent), transparent 72%),
      radial-gradient(190px 120px at 0% 100%, color-mix(in srgb, var(--c3) 13%, transparent), transparent 70%),
      radial-gradient(160px 90px at 6% 0%, color-mix(in srgb, var(--c4) 12%, transparent), transparent 70%);
    opacity: calc(0.7 + var(--lvl) * 0.45);
    transition: opacity var(--dur) ease-out;
  }

  .idle .wash {
    opacity: 0.55;
  }

  .now {
    position: relative;
    display: flex;
    align-items: flex-start;
    gap: 11px;
    min-width: 0;
  }

  .cover-button {
    flex: none;
    padding: 0;
    border: 0;
    border-radius: var(--radius-s);
    background: none;
    cursor: pointer;
    box-shadow: 0 6px 16px -6px rgba(0, 0, 0, 0.7);
    transition:
      transform var(--dur) ease-out,
      filter var(--dur) ease-out;
  }

  .cover-button:hover {
    transform: scale(1.04);
    filter: brightness(1.06);
  }

  .cover-button:active {
    transform: scale(0.97);
  }

  .meta {
    position: relative;
    flex: 1;
    min-width: 0;
    padding-top: 1px;
  }

  .title {
    font-size: 13px;
    line-height: 16px;
    font-weight: 600;
    letter-spacing: -0.005em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .idle .title {
    font-family: var(--font-display);
    font-size: 14px;
    letter-spacing: -0.01em;
  }

  .lines {
    transition: opacity var(--dur) ease-out;
  }

  .sub,
  .source {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .sub {
    margin-top: 1px;
    font-size: 11.5px;
    line-height: 15px;
    color: var(--text-dim);
  }

  .sub.wrap {
    white-space: normal;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }

  .source {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-top: 1px;
    font-size: 10.5px;
    line-height: 14px;
    color: rgba(243, 243, 245, 0.46);
    font-variant-numeric: tabular-nums;
  }

  .source span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .source::before {
    content: '';
    flex: none;
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--dot);
    box-shadow: 0 0 6px color-mix(in srgb, var(--dot) 70%, transparent);
  }

  /* A notice takes the place of the lines under the title for a few seconds. */
  .notice {
    position: absolute;
    left: 0;
    right: 0;
    top: 17px;
    margin: 0;
    font-size: 11.5px;
    line-height: 14px;
    color: var(--text);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    opacity: 0;
    transform: translateY(2px);
    pointer-events: none;
    transition:
      opacity var(--dur) ease-out,
      transform var(--dur) ease-out;
  }

  .noticed .notice {
    opacity: 1;
    transform: none;
  }

  .noticed .lines {
    opacity: 0;
  }

  /* In the corner, beside the title only: the lines below run under it. */
  .heart {
    position: absolute;
    top: -7px;
    right: -6px;
  }

  .hearted .title {
    padding-right: 22px;
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 2px;
    margin-left: -6px;
  }

  .btn {
    flex: none;
    width: 28px;
    height: 28px;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: rgba(243, 243, 245, 0.78);
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out,
      transform var(--dur) ease-out;
  }

  .btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
  }

  .btn:active:not(:disabled) {
    transform: scale(0.92);
  }

  .btn:disabled {
    color: rgba(243, 243, 245, 0.24);
    cursor: default;
  }

  .volume {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 2px;
    margin-left: 4px;
  }

  .slider {
    flex: 1;
    min-width: 36px;
    max-width: 76px;
  }

  .open {
    flex: none;
    height: 24px;
    margin-left: 8px;
    padding: 0 10px;
    border: 0;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.06);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
    color: rgba(243, 243, 245, 0.82);
    font: inherit;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.01em;
    white-space: nowrap;
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .open:hover {
    background: rgba(255, 255, 255, 0.11);
    color: var(--text);
  }

  .btn:focus-visible,
  .open:focus-visible,
  .cover-button:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 1px;
  }

  /* The ring is centred on the card's left edge, set into it with a dark bezel. Its glow must
     stay inside the window's shape (window-shape.json `mini.glow`: 22 px past the bezel,
     66 px from the centre, which is also the room to the window's left edge). With the main
     ring's blur it reaches 79 px; this tighter blur and drop end it at about 61 px (measured
     at full loudness), so nothing is cut. */
  .dock {
    position: absolute;
    left: calc(var(--pad-left) - var(--ring) / 2);
    top: calc(var(--pad-y) + var(--h) / 2 - var(--ring) / 2);
    width: var(--ring);
    height: var(--ring);
    z-index: 2;
    border-radius: 50%;
    background: var(--bg);
    box-shadow: 0 0 0 var(--bezel) var(--bg);
    --glow-blur: 9px;
    --glow-drop: 6%;
  }
</style>
