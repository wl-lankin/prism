<script lang="ts">
  import { t, tc } from '../i18n';
  import { libraryAlbumTracks, libraryArtistAlbums } from '../ipc';
  import { playIds } from '../play';
  import { shuffle } from '../flow';
  import { library } from '../stores/library.svelte';
  import { back } from '../stores/nav.svelte';
  import { notify } from '../stores/notice.svelte';
  import type { Album } from '../types';
  import AlbumGrid from './AlbumGrid.svelte';
  import Cover from './Cover.svelte';
  import Icon from './Icon.svelte';

  /** One artist: their albums in the card grid, plus play and shuffle for all of them. */
  let { name }: { name: string } = $props();

  let albums = $state.raw<Album[] | null>(null);
  let failed = $state<string | null>(null);

  $effect(() => {
    const artist = name;
    void library.version;
    let gone = false;
    failed = null;
    libraryArtistAlbums(artist)
      .then((list) => {
        if (!gone) albums = list ?? [];
      })
      .catch((err) => {
        if (!gone) failed = String(err);
      });
    return () => {
      gone = true;
    };
  });

  const info = $derived(library.artists?.find((a) => a.name === name) ?? null);
  const coverId = $derived(info?.coverId ?? albums?.find((a) => a.coverId)?.coverId ?? null);
  const trackCount = $derived(info?.trackCount ?? albums?.reduce((n, a) => n + a.trackCount, 0) ?? 0);

  async function playAll(shuffled: boolean) {
    const ids: string[] = [];
    try {
      for (const album of albums ?? []) {
        for (const track of await libraryAlbumTracks(album.id)) ids.push(track.id);
      }
    } catch (err) {
      notify(t('flow.failed', { message: String(err) }));
      return;
    }
    await playIds(shuffled ? shuffle(ids) : ids, 0);
  }
</script>

{#snippet header()}
  <button type="button" class="back" onclick={back} aria-label={t('album.back')} title={t('album.back')}>
    <Icon name="back" size={18} />
  </button>
  <header class="hero">
    <Cover {coverId} size={132} cache={300} radius="50%" alt={name} />
    <div class="info">
      <h1>{name}</h1>
      <p class="meta">
        {#if albums}
          <span>{tc('view.count.albums', albums.length)}</span>
          <span>{tc('view.count.songs', trackCount)}</span>
        {/if}
      </p>
      <div class="actions">
        <button type="button" class="btn primary" disabled={!albums?.length} onclick={() => playAll(false)}>
          <Icon name="play" size={16} filled />
          {t('album.play')}
        </button>
        <button type="button" class="btn ghost" disabled={!albums?.length} onclick={() => playAll(true)}>
          <Icon name="shuffle" size={16} />
          {t('album.shuffle')}
        </button>
      </div>
    </div>
  </header>
  {#if failed}<p class="missing">{failed}</p>{/if}
  {#if albums && albums.length === 0}<p class="missing">{t('artist.missing')}</p>{/if}
{/snippet}

<AlbumGrid albums={albums ?? []} scrollKey="artist:{name}" {header} />

<style>
  .back {
    width: 32px;
    height: 32px;
    display: grid;
    place-items: center;
    margin: 0 0 14px -6px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.05);
    color: var(--text);
    cursor: pointer;
    transition: background-color var(--dur) ease-out;
  }

  .back:hover {
    background: rgba(255, 255, 255, 0.1);
  }

  .back:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
  }

  .hero {
    display: flex;
    align-items: center;
    gap: 28px;
    margin-bottom: 34px;
  }

  .info {
    min-width: 0;
  }

  h1 {
    margin: 0;
    font-family: var(--font-display);
    font-size: clamp(30px, 3.6vw, 44px);
    font-weight: 600;
    letter-spacing: -0.025em;
    line-height: 1.05;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    display: flex;
    gap: 16px;
    margin: 8px 0 18px;
    font-size: 13px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .actions {
    display: flex;
    gap: 10px;
  }

  .missing {
    color: var(--text-dim);
    font-size: 14px;
  }
</style>
