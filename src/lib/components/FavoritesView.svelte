<script lang="ts" module>
  import type { FavKind } from '../types';

  /** The tab stays where it was left while the app runs. */
  let lastTab: FavKind = 'track';
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import { formatDuration } from '../format';
  import { t, tc, type Key } from '../i18n';
  import { favoritesAlbums, favoritesArtists, favoritesTracks, libraryAlbumTracks, libraryArtistAlbums } from '../ipc';
  import { playIds, playShuffled } from '../play';
  import { FAVORITE_KEY, favorites, isFavorite, isFavoriteKey, toggleFavorite } from '../stores/favorites.svelte';
  import { hasMixedSources, library } from '../stores/library.svelte';
  import { go } from '../stores/nav.svelte';
  import { notify } from '../stores/notice.svelte';
  import { player } from '../stores/player.svelte';
  import { loadSaved, radio } from '../stores/radio.svelte';
  import type { Album, Artist, Source, Track } from '../types';
  import { menuPoint } from '../menu';
  import AlbumGrid from './AlbumGrid.svelte';
  import TrackMenu, { type TrackMenuState } from './TrackMenu.svelte';
  import Cover from './Cover.svelte';
  import Heart from './Heart.svelte';
  import Icon from './Icon.svelte';
  import NowPlayingBars from './NowPlayingBars.svelte';
  import RowPlay from './RowPlay.svelte';
  import SourceBadge from './SourceBadge.svelte';
  import StationList from './StationList.svelte';
  import ViewHead from './ViewHead.svelte';

  /** Favorite songs, albums, artists and stations in four tabs, narrowed by source with the chips. */
  type Filter = 'all' | Source;

  let tab = $state<FavKind>(lastTab);
  let source = $state<Filter>('all');
  let selected = $state(-1);

  let tracks = $state.raw<Track[] | null>(null);
  let albums = $state.raw<Album[] | null>(null);
  let artists = $state.raw<Artist[] | null>(null);
  let failed = $state<string | null>(null);

  $effect(() => {
    void favorites.version;
    void library.version;
    let gone = false;
    Promise.all([favoritesTracks(), favoritesAlbums(), favoritesArtists()])
      .then(([tr, al, ar]) => {
        if (gone) return;
        tracks = tr ?? [];
        albums = al ?? [];
        artists = ar ?? [];
        failed = null;
      })
      .catch((err) => {
        if (!gone) failed = String(err);
      });
    return () => {
      gone = true;
    };
  });

  onMount(() => void loadSaved());

  /** A few saved stations render as plain rows; a long list only near the viewport. */
  const STATIONS_VIRTUAL_FROM = 50;

  // A station un-hearted here leaves the list at once, like the other kinds.
  const shownStations = $derived(radio.saved.favorites.filter((x) => favorites.stations.has(x.id)));

  // An item un-hearted here leaves the list at once, before the backend confirms it.
  const keepSource = <T extends { source: Source }>(item: T) => source === 'all' || item.source === source;
  const shownTracks = $derived((tracks ?? []).filter((x) => isFavorite('track', x.id) && keepSource(x)));
  const shownAlbums = $derived((albums ?? []).filter((x) => isFavorite('album', x.id) && keepSource(x)));
  // Artists are hearted by id (the name for a local artist, `navidrome:<id>` for a Navidrome
  // one), so an artist found in both sources is listed once per source.
  const shownArtists = $derived((artists ?? []).filter((x) => isFavorite('artist', x.id) && keepSource(x)));

  /** All, then every source that has favorites, Local files always among them. Stations are only Radio. */
  const sources = $derived.by((): Source[] => {
    if (tab === 'station') return ['radio'];
    const present = new Set<Source>(['local']);
    for (const item of [...(tracks ?? []), ...(albums ?? []), ...(artists ?? [])]) present.add(item.source);
    present.delete('radio');
    return [...present];
  });

  const counts = $derived({
    track: shownTracks.length,
    album: shownAlbums.length,
    artist: shownArtists.length,
    station: shownStations.length,
  });
  const loaded = $derived(tracks !== null);
  /** True when the tab has favorites, only none from the chosen source. */
  const hiddenBySource = $derived.by(() => {
    if (source === 'all' || tab === 'station') return false;
    if (tab === 'track') return (tracks ?? []).some((x) => isFavorite('track', x.id));
    if (tab === 'album') return (albums ?? []).some((x) => isFavorite('album', x.id));
    return (artists ?? []).some((x) => isFavorite('artist', x.id));
  });

  const tabs: { kind: FavKind; label: Key; count: Key; icon: 'songs' | 'album' | 'artist' | 'radio' }[] = [
    { kind: 'track', label: 'nav.songs', count: 'view.count.songs', icon: 'songs' },
    { kind: 'album', label: 'nav.albums', count: 'view.count.albums', icon: 'album' },
    { kind: 'artist', label: 'nav.artists', count: 'view.count.artists', icon: 'artist' },
    { kind: 'station', label: 'favorites.stations', count: 'favorites.count.station', icon: 'radio' },
  ];
  const current = $derived(tabs.find((x) => x.kind === tab)!);
  const playingId = $derived(player.track?.id ?? null);
  const playingArtist = $derived(player.track?.albumArtist ?? null);
  /** Items carry their source's dot once the library has more than one source. */
  const mixed = $derived(hasMixedSources());

  function choose(kind: FavKind) {
    tab = kind;
    lastTab = kind;
    selected = -1;
    // Radio only exists on the stations tab, and only Radio does there.
    if ((kind === 'station') !== (source === 'radio') && source !== 'all') source = 'all';
  }

  function onTabKey(event: KeyboardEvent) {
    const i = tabs.findIndex((x) => x.kind === tab);
    const moves: Record<string, number> = { ArrowRight: i + 1, ArrowLeft: i - 1, Home: 0, End: tabs.length - 1 };
    if (!(event.key in moves)) return;
    event.preventDefault();
    const next = tabs[(moves[event.key] + tabs.length) % tabs.length];
    choose(next.kind);
    document.getElementById(`fav-tab-${next.kind}`)?.focus();
  }

  /** Plays the listed songs (as filtered) from row `index`. */
  function play(index: number, shuffled = false) {
    const ids = shownTracks.map((x) => x.id);
    void (shuffled ? playShuffled(ids) : playIds(ids, index));
  }

  /** Plays everything the current tab lists: the songs, or the tracks of the albums or artists. */
  async function playTab(shuffled: boolean) {
    if (tab === 'track') return play(0, shuffled);
    const ids: string[] = [];
    try {
      // An artist's albums come from every source. Only those of a shown favorite artist of the
      // same source play: a local favorite does not pull in that name's Navidrome albums.
      const pair = (name: string, source: Source) => `${name}\u0000${source}`;
      const pairs = new Set(shownArtists.map((x) => pair(x.name, x.source)));
      const names = [...new Set(shownArtists.map((x) => x.name))];
      const albumIds =
        tab === 'album'
          ? shownAlbums.map((x) => x.id)
          : (await Promise.all(names.map(async (name) => ({ name, list: await libraryArtistAlbums(name) }))))
              .flatMap(({ name, list }) => (list ?? []).filter((x) => pairs.has(pair(name, x.source))))
              .map((x) => x.id);
      for (const list of await Promise.all(albumIds.map((id) => libraryAlbumTracks(id)))) {
        for (const track of list) ids.push(track.id);
      }
    } catch (err) {
      notify(t('flow.failed', { message: String(err) }));
      return;
    }
    await (shuffled ? playShuffled(ids) : playIds(ids, 0));
  }

  function rowIndex(event: MouseEvent): number | null {
    const target = event.target as HTMLElement;
    if (target.closest('button')) return null;
    const row = target.closest<HTMLElement>('[data-index]');
    return row ? Number(row.dataset.index) : null;
  }

  // A row un-hearted at the end of the list leaves the selection past it.
  $effect(() => {
    if (selected >= shownTracks.length) selected = shownTracks.length - 1;
  });

  let menu = $state<TrackMenuState | null>(null);

  /** Right-click (or the context menu key) on a song: select it and open the track menu. */
  function onListMenu(event: MouseEvent) {
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-index]');
    const i = row ? Number(row.dataset.index) : selected;
    const track = shownTracks[i];
    if (!track) return;
    event.preventDefault();
    selected = i;
    const at = menuPoint(event, document.getElementById(`fav-song-${i}`));
    menu = { trackIds: [track.id], ...at, albumId: track.albumId, artist: track.albumArtist || track.artist };
  }

  function onListKey(event: KeyboardEvent) {
    const n = shownTracks.length;
    if (!n) return;
    const moves: Record<string, number> = { ArrowDown: selected + 1, ArrowUp: selected - 1, Home: 0, End: n - 1 };
    if (event.key in moves) {
      event.preventDefault();
      selected = Math.max(0, Math.min(n - 1, moves[event.key]));
      document.getElementById(`fav-song-${selected}`)?.scrollIntoView({ block: 'nearest' });
    } else if (event.key === 'Enter' && selected >= 0) {
      event.preventDefault();
      play(selected);
    } else if (isFavoriteKey(event) && shownTracks[selected]) {
      // Un-hearting drops the row; the selection stays on the row that moves up into its place.
      event.preventDefault();
      void toggleFavorite('track', shownTracks[selected].id);
    }
  }

  function artistCounts(artist: Artist): string {
    return `${tc('view.count.albums', artist.albumCount)}, ${tc('view.count.songs', artist.trackCount)}`;
  }
</script>

{#snippet actions()}
  <button type="button" class="btn primary" disabled={!counts[tab]} onclick={() => playTab(false)}>
    <Icon name="play" size={16} filled />
    {t('album.play')}
  </button>
  <button type="button" class="btn ghost" disabled={!counts[tab]} onclick={() => playTab(true)}>
    <Icon name="shuffle" size={16} />
    {t('album.shuffle')}
  </button>
{/snippet}

{#snippet header()}
  <!-- Stations cannot be queued, so their tab has no Play and Shuffle. -->
  <ViewHead
    title={t('nav.favorites')}
    meta={loaded ? tc(current.count, counts[tab]) : undefined}
    actions={tab === 'station' ? undefined : actions}
  />

  <div class="bar">
    <div class="tabs" role="tablist" aria-label={t('favorites.kinds')} tabindex="-1" onkeydown={onTabKey}>
      {#each tabs as item (item.kind)}
        <button
          type="button"
          role="tab"
          id="fav-tab-{item.kind}"
          class="tab"
          class:active={tab === item.kind}
          aria-selected={tab === item.kind}
          aria-controls="fav-panel"
          tabindex={tab === item.kind ? 0 : -1}
          onclick={() => choose(item.kind)}
        >
          <Icon name={item.icon} size={16} />
          <span>{t(item.label)}</span>
          {#if loaded}<span class="count">{counts[item.kind].toLocaleString()}</span>{/if}
        </button>
      {/each}
    </div>

    <div class="chips" role="radiogroup" aria-label={t('nav.sources')}>
      {#each ['all', ...sources] as chip (chip)}
        <button
          type="button"
          role="radio"
          class="chip"
          class:active={source === chip}
          aria-checked={source === chip}
          onclick={() => (source = chip as Filter)}
        >
          {#if chip !== 'all'}<span class="dot" style="--dot:var(--src-{chip})"></span>{/if}
          {chip === 'all' ? t('favorites.all') : chip === 'radio' ? t('favorites.chip.radio') : t(`source.${chip}`)}
        </button>
      {/each}
    </div>
  </div>

  {#if failed}<p class="failed">{t('favorites.loadFailed', { message: failed })}</p>{/if}

{/snippet}

{#snippet empty()}
  <div class="empty">
    {#if hiddenBySource}
      <p class="lead">{t('favorites.emptySource', { source: t(`source.${source}`) })}</p>
      <button type="button" class="btn ghost" onclick={() => (source = 'all')}>{t('favorites.all')}</button>
    {:else}
      <span class="mark" aria-hidden="true"><Icon name="heart" size={26} /></span>
      <h2>{t(`favorites.empty.${tab}`)}</h2>
      <p class="lead">{t(`favorites.empty.${tab}Lead`)}</p>
      {#if tab === 'station'}
        <button type="button" class="btn ghost" onclick={() => go('radio', 'near')}>
          <Icon name="radio" size={16} />
          {t('favorites.empty.stationAction')}
        </button>
      {:else}
        <button type="button" class="btn ghost" onclick={() => go('albums')}>
          <Icon name="album" size={16} />
          {t('favorites.browse')}
        </button>
      {/if}
    {/if}
  </div>
{/snippet}

<div class="panel" id="fav-panel" role="tabpanel" aria-labelledby="fav-tab-{tab}">
  {#if tab === 'album' && shownAlbums.length}
    {#key source}
      <AlbumGrid albums={shownAlbums} scrollKey="favorites:albums" {header} showSource={mixed} />
    {/key}
  {:else}
    <div class="scroll">
      {@render header()}

      {#if tab === 'station'}
        {#if shownStations.length}
          <div class="stations">
            <StationList stations={shownStations} virtual={shownStations.length >= STATIONS_VIRTUAL_FROM} />
          </div>
        {:else}
          {@render empty()}
        {/if}
      {:else if !loaded}
        <!-- The first load takes a moment; nothing flashes meanwhile. -->
      {:else if counts[tab] === 0}
        {@render empty()}
      {:else if tab === 'track'}
        <div
          class="songs"
          role="listbox"
          tabindex="0"
          aria-keyshortcuts={FAVORITE_KEY}
          aria-label={t('nav.favorites')}
          aria-activedescendant={selected >= 0 ? `fav-song-${selected}` : undefined}
          onkeydown={onListKey}
          oncontextmenu={onListMenu}
          onclick={(event) => {
            const i = rowIndex(event);
            if (i !== null) selected = i;
          }}
          ondblclick={(event) => {
            const i = rowIndex(event);
            if (i !== null) play(i);
          }}
        >
          {#each shownTracks as track, i (track.id)}
            {@const playing = track.id === playingId}
            <div
              id="fav-song-{i}"
              class="row"
              class:selected={selected === i}
              class:playing
              role="option"
              aria-selected={selected === i}
              data-index={i}
            >
              <span class="num">
                <RowPlay number={i + 1} name={track.title} current={playing} revealed={selected === i} onplay={() => play(i)} />
              </span>
              <span class="art">
                <Cover coverId={track.coverId} size={38} cache={96} radius="7px" />
                {#if mixed && track.source !== 'local'}<SourceBadge source={track.source} size={9} />{/if}
              </span>
              <span class="name">
                <span class="title">{track.title}</span>
                <button type="button" class="sub link" tabindex="-1" onclick={() => go('artist', track.albumArtist || track.artist)}>
                  {track.artist}
                </button>
              </span>
              <button type="button" class="album link" tabindex="-1" onclick={() => go('album', track.albumId)}>
                {track.album}
              </button>
              <Heart kind="track" id={track.id} size={16} tabindex={-1} label={t('favorites.toggleItem', { name: track.title })} />
              <span class="time">{formatDuration(track.durationMs)}</span>
            </div>
          {/each}
        </div>
      {:else if tab === 'artist'}
        <ul class="artists">
          {#each shownArtists as artist (`${artist.source}:${artist.id}`)}
            <li class="artist">
              <button type="button" class="open" onclick={() => go('artist', artist.name)}>
                <span class="portrait">
                  <Cover coverId={artist.coverId} size={148} cache={300} radius="50%" alt="" />
                  {#if mixed && artist.source !== 'local'}<SourceBadge source={artist.source} size={12} />{/if}
                </span>
                <span class="who">
                  {#if playingArtist === artist.name}<NowPlayingBars playing={player.status === 'playing'} size={11} />{/if}
                  <span class="text">{artist.name}</span>
                </span>
                <span class="counts">{artistCounts(artist)}</span>
              </button>
              <span class="pin">
                <Heart kind="artist" id={artist.id} size={16} label={t('favorites.toggleItem', { name: artist.name })} />
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</div>

{#if menu}
  <TrackMenu {...menu} onclose={() => (menu = null)} />
{/if}

<style>
  .panel {
    height: 100%;
  }

  .scroll {
    scrollbar-color: auto;
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    scrollbar-gutter: stable;
    padding: 4px 32px 40px;
    box-sizing: border-box;
  }

  /* Tabs on the left, source chips on the right, one line under the title. */
  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px 20px;
    margin: -6px 0 26px;
  }

  .tabs {
    display: inline-flex;
    gap: 2px;
    padding: 3px;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.04);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.05);
    outline: none;
  }

  .tab {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 13px 0 11px;
    border: 0;
    border-radius: 9px;
    background: transparent;
    color: rgba(243, 243, 245, 0.6);
    font: inherit;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out,
      box-shadow var(--dur) ease-out;
  }

  .tab:hover {
    color: var(--text);
  }

  .tab.active {
    background: var(--raised);
    color: var(--text);
    box-shadow:
      inset 0 0 0 1px rgba(255, 255, 255, 0.08),
      0 4px 12px -6px rgba(0, 0, 0, 0.7);
  }

  .tab:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 1px;
  }

  .count {
    min-width: 18px;
    padding: 0 5px;
    border-radius: 5px;
    font-size: 11px;
    font-weight: 600;
    line-height: 17px;
    text-align: center;
    font-variant-numeric: tabular-nums;
    color: rgba(243, 243, 245, 0.5);
    background: rgba(255, 255, 255, 0.06);
  }

  .tab.active .count {
    color: var(--spec-1);
    background: color-mix(in srgb, var(--spec-1) 14%, transparent);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 28px;
    padding: 0 12px;
    border: 0;
    border-radius: 999px;
    background: transparent;
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.1);
    color: var(--text-dim);
    font: inherit;
    font-size: 12.5px;
    font-weight: 500;
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out,
      box-shadow var(--dur) ease-out;
  }

  .chip:hover {
    color: var(--text);
    background: rgba(255, 255, 255, 0.04);
  }

  .chip.active {
    color: var(--bg);
    background: var(--text);
    box-shadow: none;
  }

  .chip:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--dot);
    box-shadow: 0 0 8px color-mix(in srgb, var(--dot) 55%, transparent);
  }

  .chip.active .dot {
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.25);
  }

  .failed {
    margin: 0 0 18px;
    font-size: 13px;
    color: var(--text-dim);
  }

  /* Songs */

  .songs {
    outline: none;
    margin: 0 -12px;
  }

  .row {
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

  .songs:focus-visible .row.selected {
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

  .sub {
    font-size: 12.5px;
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

  /* Artists: round portraits, the heart pinned to each one's lower right. */

  .artists {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(156px, 1fr));
    gap: 28px 24px;
  }

  .artist {
    position: relative;
    min-width: 0;
  }

  .open {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: center;
    cursor: pointer;
    border-radius: 14px;
  }

  .open:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 4px;
  }

  .art {
    position: relative;
    display: block;
    width: 38px;
    height: 38px;
  }

  .art :global(.source-badge) {
    position: absolute;
    right: -2px;
    bottom: -2px;
  }

  /* On the portrait's edge like a presence dot, lower left: the heart (.pin) holds the lower right. */
  .portrait :global(.source-badge) {
    position: absolute;
    left: 14px;
    bottom: 14px;
  }

  .portrait {
    position: relative;
    display: block;
    border-radius: 50%;
    box-shadow:
      0 18px 34px -18px rgba(0, 0, 0, 0.95),
      0 0 0 1px rgba(255, 255, 255, 0.06);
    transition: filter var(--dur) ease-out;
  }

  .artist:hover .portrait {
    filter: brightness(1.08);
  }

  .who {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    max-width: 100%;
    margin-top: 13px;
    font-size: 14px;
    font-weight: 560;
  }

  .text,
  .counts {
    max-width: 100%;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .counts {
    margin-top: 3px;
    font-size: 12px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .pin {
    position: absolute;
    top: 112px;
    left: calc(50% + 40px);
    border-radius: 50%;
    background: var(--bg);
    box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.08);
  }

  /* Stations: the radio view's rows, flush with the song list's edge. */

  .stations {
    margin: 0 -12px;
  }

  /* Empty tab: an invitation, aligned with the list it stands in for. */

  .empty {
    max-width: 420px;
    padding: 28px 0 12px;
  }

  .mark {
    width: 52px;
    height: 52px;
    display: grid;
    place-items: center;
    margin-bottom: 18px;
    border-radius: 50%;
    color: var(--spec-1);
    background: color-mix(in srgb, var(--spec-1) 10%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--spec-1) 28%, transparent);
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
</style>
