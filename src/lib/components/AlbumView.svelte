<script lang="ts">
  import { formatDuration, qualityLabel } from '../format';
  import { t, tc } from '../i18n';
  import { coverUrl, libraryAlbumTracks } from '../ipc';
  import { playIds } from '../play';
  import { shuffle } from '../flow';
  import { library } from '../stores/library.svelte';
  import { back, go } from '../stores/nav.svelte';
  import { player } from '../stores/player.svelte';
  import type { Track } from '../types';
  import Cover from './Cover.svelte';
  import Icon from './Icon.svelte';
  import NowPlayingBars from './NowPlayingBars.svelte';

  /** One album: a large cover over a backdrop blurred from it, play and shuffle, the tracks. */
  let { albumId }: { albumId: string } = $props();

  let tracks = $state.raw<Track[] | null>(null);
  let failed = $state<string | null>(null);
  let selected = $state(-1);

  $effect(() => {
    const id = albumId;
    void library.version;
    let gone = false;
    failed = null;
    libraryAlbumTracks(id)
      .then((list) => {
        if (!gone) tracks = list ?? [];
      })
      .catch((err) => {
        if (!gone) failed = String(err);
      });
    return () => {
      gone = true;
    };
  });

  const album = $derived(library.albums?.find((a) => a.id === albumId) ?? null);
  const first = $derived(tracks?.[0] ?? null);
  const title = $derived(album?.title ?? first?.album ?? '');
  const artist = $derived(album?.artist ?? first?.albumArtist ?? '');
  const year = $derived(album?.year ?? first?.year ?? null);
  const coverId = $derived(album?.coverId ?? first?.coverId ?? null);
  const totalMs = $derived(tracks?.reduce((sum, track) => sum + track.durationMs, 0) ?? 0);
  const discs = $derived(new Set(tracks?.map((track) => track.discNo ?? 1)).size > 1);
  const quality = $derived.by(() => {
    const labels = new Set(tracks?.map((track) => qualityLabel(track)));
    return labels.size === 1 ? [...labels][0] : null;
  });
  const ids = $derived(tracks?.map((track) => track.id) ?? []);
  const playingId = $derived(player.track?.id ?? null);
  const backdrop = $derived(coverUrl(coverId, 96));

  function length(ms: number): string {
    const minutes = Math.max(1, Math.round(ms / 60000));
    if (minutes < 60) return t('time.minutes', { m: minutes });
    return t('time.hours', { h: Math.floor(minutes / 60), m: minutes % 60 });
  }

  function play(index: number) {
    void playIds(ids, index);
  }

  /** The row a click landed on (rows carry `data-index`). */
  function rowIndex(event: MouseEvent): number | null {
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-index]');
    return row ? Number(row.dataset.index) : null;
  }

  function onkeydown(event: KeyboardEvent) {
    if (!tracks?.length) return;
    const moves: Record<string, number> = {
      ArrowDown: selected + 1,
      ArrowUp: selected - 1,
      Home: 0,
      End: tracks.length - 1,
    };
    if (event.key in moves) {
      event.preventDefault();
      selected = Math.max(0, Math.min(tracks.length - 1, moves[event.key]));
      document.getElementById(`track-${selected}`)?.scrollIntoView({ block: 'nearest' });
    } else if (event.key === 'Enter' && selected >= 0) {
      event.preventDefault();
      play(selected);
    }
  }
</script>

<div class="scroll">
  {#if backdrop}
    <div class="backdrop" aria-hidden="true">
      <img src={backdrop} alt="" draggable="false" />
    </div>
  {/if}

  <div class="inner">
    <button type="button" class="back" onclick={back} aria-label={t('album.back')} title={t('album.back')}>
      <Icon name="back" size={18} />
    </button>

    {#if failed}
      <p class="missing">{failed}</p>
    {:else if tracks && tracks.length === 0}
      <p class="missing">{t('album.missing')}</p>
    {:else}
      <header class="hero">
        <div class="art">
          <Cover {coverId} size={232} cache={600} radius="14px" alt={title} />
        </div>
        <div class="info">
          <h1>{title}</h1>
          {#if artist}
            <button type="button" class="artist" onclick={() => go('artist', artist)}>{artist}</button>
          {/if}
          <p class="meta">
            {#if year}<span>{year}</span>{/if}
            {#if tracks}
              <span>{tc('view.count.tracks', tracks.length)}</span>
              <span>{length(totalMs)}</span>
            {/if}
            {#if quality}<span class="chip">{quality}</span>{/if}
          </p>
          <div class="actions">
            <button type="button" class="btn primary" disabled={!ids.length} onclick={() => play(0)}>
              <Icon name="play" size={16} filled />
              {t('album.play')}
            </button>
            <button type="button" class="btn ghost" disabled={!ids.length} onclick={() => playIds(shuffle([...ids]), 0)}>
              <Icon name="shuffle" size={16} />
              {t('album.shuffle')}
            </button>
          </div>
        </div>
      </header>

      {#if tracks}
        <div
          class="tracks"
          role="listbox"
          tabindex="0"
          aria-label={title}
          aria-activedescendant={selected >= 0 ? `track-${selected}` : undefined}
          {onkeydown}
          onclick={(event) => {
            const i = rowIndex(event);
            if (i !== null) selected = i;
          }}
          ondblclick={(event) => {
            const i = rowIndex(event);
            if (i !== null) play(i);
          }}
        >
          {#each tracks as track, i (track.id)}
            {@const playing = track.id === playingId}
            {#if discs && (i === 0 || (tracks[i - 1].discNo ?? 1) !== (track.discNo ?? 1))}
              <div class="disc" role="presentation">{t('album.disc', { n: track.discNo ?? 1 })}</div>
            {/if}
            <div
              id="track-{i}"
              class="row"
              class:selected={selected === i}
              class:playing
              role="option"
              aria-selected={selected === i}
              data-index={i}
            >
              <span class="num">
                {#if playing}
                  <NowPlayingBars playing={player.status === 'playing'} size={12} />
                {:else}
                  {track.trackNo ?? i + 1}
                {/if}
              </span>
              <span class="name">
                <span class="title">{track.title}</span>
                {#if track.artist && track.artist !== artist}
                  <span class="feat">{track.artist}</span>
                {/if}
              </span>
              <span class="time">{formatDuration(track.durationMs)}</span>
            </div>
          {/each}
        </div>
      {/if}
    {/if}
  </div>
</div>

<style>
  .scroll {
    scrollbar-color: auto;
    position: relative;
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
  }

  /* The cover, tiny and blurred far past recognition, tints the top of the view. */
  .backdrop {
    position: absolute;
    inset: 0 0 auto 0;
    height: 520px;
    overflow: hidden;
    pointer-events: none;
    /* Fades in under the title bar and out into the track list, so it has no hard edge. */
    mask-image: linear-gradient(
      to bottom,
      rgba(0, 0, 0, 0.25) 0,
      #000 110px,
      rgba(0, 0, 0, 0.55) 55%,
      transparent 100%
    );
  }

  .backdrop img {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 100%;
    height: 100%;
    object-fit: cover;
    transform: translate(-50%, -50%) scale(1.5);
    filter: blur(60px) saturate(1.35);
    opacity: 0.55;
    animation: fade var(--dur-l) ease-out;
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
    min-width: 0;
    padding-bottom: 4px;
  }

  h1 {
    margin: 0 0 8px;
    font-family: var(--font-display);
    font-size: clamp(30px, 3.6vw, 46px);
    font-weight: 600;
    letter-spacing: -0.025em;
    line-height: 1.04;
    text-wrap: balance;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .artist {
    padding: 0;
    border: 0;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 16px;
    font-weight: 500;
    cursor: pointer;
  }

  .artist:hover {
    text-decoration: underline;
    text-underline-offset: 4px;
    text-decoration-color: rgba(243, 243, 245, 0.4);
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

  .chip {
    padding: 2px 8px;
    border-radius: 6px;
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: var(--text);
    background: rgba(255, 255, 255, 0.08);
  }

  .actions {
    display: flex;
    gap: 10px;
  }

  .tracks {
    outline: none;
    margin: 0 -12px;
  }

  .disc {
    padding: 18px 12px 8px;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text-dim);
  }

  .row {
    display: grid;
    grid-template-columns: 32px minmax(0, 1fr) 56px;
    column-gap: 16px;
    align-items: center;
    height: 44px;
    padding: 0 12px;
    border-radius: 8px;
    font-size: 14px;
    cursor: default;
    transition: background-color var(--dur) ease-out;
  }

  .row:hover {
    background: rgba(255, 255, 255, 0.04);
  }

  .row.selected {
    background: rgba(255, 255, 255, 0.075);
  }

  .tracks:focus-visible .row.selected {
    box-shadow: inset 0 0 0 1.5px rgba(77, 166, 255, 0.7);
  }

  .num,
  .time {
    display: flex;
    justify-content: flex-end;
    font-size: 12.5px;
    color: rgba(243, 243, 245, 0.45);
    font-variant-numeric: tabular-nums;
  }

  .name {
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
  }

  .title,
  .feat {
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

  .feat {
    font-size: 13px;
    color: var(--text-dim);
  }

  .missing {
    color: var(--text-dim);
    font-size: 14px;
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
