<script lang="ts">
  import { formatDuration, isLive, liveLines, qualityLabel, streamedFrom } from '../format';
  import { t } from '../i18n';
  import { playerNext, playerPrev, playerSeek, playerSetVolume } from '../ipc';
  import { back, go, nav } from '../stores/nav.svelte';
  import { player } from '../stores/player.svelte';
  import { currentTitle } from '../stores/radio.svelte';
  import { settings } from '../stores/settings.svelte';
  import { QUEUE_TOGGLE_ID } from '../queue';
  import { queuePanel, toggleQueuePanel } from '../stores/queue.svelte';
  import Cover from './Cover.svelte';
  import Heart from './Heart.svelte';
  import Icon from './Icon.svelte';
  import LivePill from './LivePill.svelte';
  import PlayModes from './PlayModes.svelte';
  import Slider from './Slider.svelte';
  import StationTile from './StationTile.svelte';

  /** The bottom strip of the content card. The centre keeps `ringGap` px free for the Flow
   *  ring, which the window places over it. `ringInside` is set when the ring sits centred
   *  in the strip (maximized window) instead of on the card's bottom edge. */
  let { ringGap = 132, ringInside = false }: { ringGap?: number; ringInside?: boolean } = $props();

  const track = $derived(player.track);
  const idle = $derived(player.status === 'idle' || !track);
  const duration = $derived(player.durationMs || track?.durationMs || 0);
  /** A station: no seek bar, no duration, no next or previous. */
  const live = $derived(!idle && isLive(track));
  const lines = $derived(track ? liveLines(track, live ? currentTitle(track) : null) : null);

  /** How the track plays; a Navidrome transcode names the file behind it in the tooltip. */
  const quality = $derived(track ? qualityLabel(track, settings.navidromeQuality) : '');
  const streamNote = $derived.by(() => {
    const file = track ? streamedFrom(track, settings.navidromeQuality) : null;
    return file ? t('quality.streamedFrom', { stream: quality, file }) : undefined;
  });

  // While the seek bar is dragged it shows the drag position, not the engine's.
  let seeking = $state<number | null>(null);
  const position = $derived(seeking ?? player.positionMs);

  let lastVolume = 0.8;
  const muted = $derived(player.volume <= 0.001);
  const volumeIcon = $derived(muted ? 'mute' : player.volume < 0.5 ? 'volumeLow' : 'volume');

  function report(what: string) {
    return (err: unknown) => console.error(`${what} failed`, err);
  }

  function setVolume(v: number) {
    player.volume = v;
    playerSetVolume(v).catch(report('setting the volume'));
  }

  /** The cover and title open Now Playing; on it, they go back to where you were. */
  const openLabel = $derived(nav.view === 'nowplaying' ? t('nowPlaying.close') : t('nowPlaying.open'));

  function toggleNowPlaying() {
    if (nav.view === 'nowplaying') back();
    else go('nowplaying');
  }

  function toggleMute() {
    if (muted) setVolume(lastVolume > 0.001 ? lastVolume : 0.5);
    else {
      lastVolume = player.volume;
      setVolume(0);
    }
  }
</script>

<div class="strip" class:inside={ringInside} style="--gap:{ringGap}px">
  <div class="seek" class:live>
    {#if live}
      <span class="live-rail" aria-hidden="true"></span>
    {:else}
      <Slider
        value={duration ? position / duration : 0}
        label={t('transport.seek')}
        valueText={`${formatDuration(position)} / ${formatDuration(duration)}`}
        step={duration ? 5000 / duration : 0.05}
        spectrum
        disabled={idle || !duration}
        oninput={(v) => (seeking = v * duration)}
        oncancel={() => (seeking = null)}
        oncommit={(v) => {
          seeking = null;
          player.positionMs = v * duration;
          playerSeek(v * duration).catch(report('seeking'));
        }}
      />
    {/if}
  </div>

  <div class="now">
    {#if idle}
      <Cover coverId={track?.coverId ?? null} size={48} alt="" />
    {:else}
      <!-- The title below is the keyboard stop; the cover is the same target for the pointer. -->
      <button type="button" class="open cover" tabindex="-1" aria-label={openLabel} title={openLabel} onclick={toggleNowPlaying}>
        {#if live && track}
          <StationTile name={track.title} coverId={track.coverId} size={48} radius="var(--radius-s)" />
        {:else}
          <Cover coverId={track?.coverId ?? null} size={48} alt="" />
        {/if}
      </button>
    {/if}
    <div class="meta">
      {#if idle}
        <div class="title">{t('transport.idleTitle')}</div>
        <div class="sub">{t('transport.idleHint')}</div>
      {:else if track && lines}
        <button type="button" class="open title" title={openLabel} aria-pressed={nav.view === 'nowplaying'} onclick={toggleNowPlaying}>{lines.title}</button>
        <div class="sub" title={lines.sub}>
          {player.status === 'loading' ? t('transport.loading') : lines.sub}
        </div>
      {/if}
    </div>
    {#if !idle && track}
      <span class="quality" style="--dot:var(--src-{track.source})" title={streamNote}>{quality}</span>
    {/if}
  </div>

  <div class="center">
    <span class="time elapsed" class:dim={idle} title={live ? t('radio.liveHeard', { time: formatDuration(position) }) : undefined}>{formatDuration(position)}</span>
    <button type="button" class="btn" disabled={idle || live} aria-label={t('transport.previous')} title={live ? t('radio.noSkip') : t('transport.previous')} onclick={() => playerPrev().catch(report('previous'))}>
      <Icon name="prev" size={20} />
    </button>
    <span class="ring-space" aria-hidden="true"></span>
    <button type="button" class="btn" disabled={idle || live} aria-label={t('transport.next')} title={live ? t('radio.noSkip') : t('transport.next')} onclick={() => playerNext().catch(report('next'))}>
      <Icon name="next" size={20} />
    </button>
    {#if live}
      <span class="time live-slot"><LivePill size="small" /></span>
    {:else}
      <span class="time" class:dim={idle}>{formatDuration(duration)}</span>
    {/if}
  </div>

  <div class="right">
    <PlayModes />
    <button
      type="button"
      id={QUEUE_TOGGLE_ID}
      class="btn small toggle"
      class:on={queuePanel.open}
      aria-pressed={queuePanel.open}
      aria-label={t('transport.queue')}
      title={t('transport.queue')}
      aria-keyshortcuts="Control+Q"
      onclick={() => toggleQueuePanel()}
    >
      <Icon name="queue" size={18} />
    </button>
    <span class="divider" aria-hidden="true"></span>
    <!-- Dropped files ("file:" ids) are not in the library; a favorite of one would be an orphan. -->
    {#if !idle && track && !track.id.startsWith('file:')}
      <Heart kind={live ? 'station' : 'track'} id={track.id} label={t('favorites.toggleItem', { name: track.title })} />
    {/if}
    <button type="button" class="btn small" aria-label={muted ? t('transport.unmute') : t('transport.mute')} title={muted ? t('transport.unmute') : t('transport.mute')} onclick={toggleMute}>
      <Icon name={volumeIcon} size={18} />
    </button>
    <div class="volume">
      <Slider value={player.volume} label={t('transport.volume')} valueText={`${Math.round(player.volume * 100)} %`} oninput={setVolume} oncommit={setVolume} />
    </div>
  </div>
</div>

<style>
  .strip {
    position: relative;
    /* 84 px with the padding below. */
    height: 76px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
    align-items: center;
    gap: 24px;
    /* All three groups share one centre line: the middle of the strip below the seek bar,
       whose 16 px control reaches 8 px into the strip. With the ring hanging out of the card's
       edge, that line also sits closer to the ring's centre. Maximized, the ring is centred in
       the whole strip, so the groups are too. */
    padding: 8px 20px 0;
    border-top: 1px solid var(--line);
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.015), rgba(255, 255, 255, 0));
  }

  .strip.inside {
    height: 84px;
    padding-top: 0;
  }

  /* The seek bar is the divider between content and transport. */
  .seek {
    position: absolute;
    left: 0;
    right: 0;
    top: -8.5px;
    z-index: 2;
  }

  /* A station: a steady spectral hairline in place of the seek rail; the pill takes the
     duration's place. */
  .live-rail {
    position: absolute;
    left: 0;
    right: 0;
    top: 7px;
    height: 2px;
    background: linear-gradient(90deg, var(--spec-1), var(--spec-2), var(--spec-3), var(--spec-4));
    opacity: 0.55;
  }

  .live-slot {
    display: inline-flex;
    align-items: center;
  }

  .now {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }

  .meta {
    min-width: 0;
    display: grid;
    gap: 3px;
  }

  .title,
  .sub {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title {
    font-size: 13.5px;
    font-weight: 600;
    letter-spacing: -0.005em;
  }

  .open {
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }

  .open.title {
    display: block;
    max-width: 100%;
    text-align: left;
    font-size: 13.5px;
    font-weight: 600;
    text-decoration: underline;
    text-decoration-color: transparent;
    text-underline-offset: 3px;
    transition: text-decoration-color var(--dur) ease-out;
  }

  .open.title:hover {
    text-decoration-color: rgba(243, 243, 245, 0.4);
  }

  .open.cover {
    flex: none;
    border-radius: var(--radius-s);
    transition: transform var(--dur) ease-out;
  }

  .open.cover:hover {
    transform: scale(1.04);
  }

  .open:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
    border-radius: 4px;
  }

  .sub {
    font-size: 12.5px;
    color: var(--text-dim);
  }

  .quality {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 22px;
    padding: 0 8px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.05);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
    color: var(--text-dim);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.02em;
    font-variant-numeric: tabular-nums;
  }

  .quality::before {
    content: '';
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--dot);
  }

  .center {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .ring-space {
    width: var(--gap);
  }

  .time {
    width: 52px;
    font-size: 11.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .time.elapsed {
    text-align: right;
  }

  .time.dim {
    opacity: 0.45;
  }

  .right {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
    min-width: 0;
  }

  .volume {
    width: 104px;
  }

  .divider {
    width: 1px;
    height: 18px;
    margin: 0 2px;
    background: var(--line);
  }

  .btn.toggle {
    position: relative;
    color: rgba(243, 243, 245, 0.62);
  }

  .btn.toggle.on {
    color: var(--text);
  }

  /* Open, it carries the same sliver of spectrum as an active play mode. */
  .btn.toggle::after {
    content: '';
    position: absolute;
    left: 50%;
    bottom: 4px;
    width: 12px;
    height: 2px;
    border-radius: 2px;
    background: linear-gradient(90deg, var(--spec-1), var(--spec-2), var(--spec-3), var(--spec-4));
    opacity: 0;
    transform: translateX(-50%) scaleX(0.4);
    transition:
      opacity var(--dur) ease-out,
      transform var(--dur) ease-out;
  }

  .btn.toggle.on::after {
    opacity: 1;
    transform: translateX(-50%) scaleX(1);
  }

  .btn {
    width: 40px;
    height: 40px;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: rgba(243, 243, 245, 0.82);
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out,
      transform var(--dur) ease-out;
  }

  .btn.small {
    width: 34px;
    height: 34px;
  }

  .btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.07);
    color: var(--text);
  }

  .btn:active:not(:disabled) {
    transform: scale(0.94);
  }

  .btn:disabled {
    color: rgba(243, 243, 245, 0.25);
    cursor: default;
  }

  .btn:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 0;
  }
</style>
