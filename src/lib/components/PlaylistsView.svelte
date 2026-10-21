<script lang="ts">
  import { t, tc } from '../i18n';
  import { playPlaylist } from '../play';
  import { go } from '../stores/nav.svelte';
  import { player } from '../stores/player.svelte';
  import { createPlaylist, playlists, suggestName } from '../stores/playlists.svelte';
  import { saveScroll, savedScroll } from '../scroll';
  import Icon from './Icon.svelte';
  import PlaylistCover from './PlaylistCover.svelte';
  import { playRemotePlaylist, remoteSeed } from './PlaylistView.svelte';
  import ViewHead from './ViewHead.svelte';

  /** Every playlist as a card with its cover mosaic. "New playlist" creates one with a free
   *  name and opens it with the title ready to type over. The Navidrome server's playlists
   *  follow in their own section, marked read-only. */
  const KEY = 'playlists';
  let scroller = $state<HTMLDivElement>();
  let creating = $state(false);

  const list = $derived(playlists.list);
  const remote = $derived(playlists.remote ?? []);

  $effect(() => {
    if (!scroller || !list) return;
    const top = savedScroll(KEY);
    if (top) scroller.scrollTop = top;
  });

  async function newPlaylist() {
    if (creating) return;
    creating = true;
    const created = await createPlaylist(suggestName());
    creating = false;
    if (!created) return;
    playlists.renaming = created.id;
    go('playlist', String(created.id));
  }

  function length(ms: number): string {
    const minutes = Math.max(1, Math.round(ms / 60000));
    if (minutes < 60) return t('time.minutes', { m: minutes });
    return t('time.hours', { h: Math.floor(minutes / 60), m: minutes % 60 });
  }
</script>

<div
  class="scroll"
  bind:this={scroller}
  onscroll={() => scroller && saveScroll(KEY, scroller.scrollTop)}
>
  <ViewHead title={t('nav.playlists')} meta={list?.length ? tc('view.count.playlists', list.length) : undefined}>
    {#snippet actions()}
      {#if list?.length}
        <button type="button" class="btn ghost" disabled={creating} onclick={newPlaylist}>
          <Icon name="plus" size={16} />
          {t('playlists.new')}
        </button>
      {/if}
    {/snippet}
  </ViewHead>

  {#if list === null}
    <!-- The first load takes a moment; nothing flashes meanwhile. -->
  {:else if list.length === 0}
    <div class="empty">
      <div class="stack" aria-hidden="true">
        <span class="sheet s1"></span>
        <span class="sheet s2"></span>
        <span class="sheet s3"><Icon name="playlists" size={26} /></span>
      </div>
      <h2>{t('playlists.empty')}</h2>
      <p class="lead">{t('playlists.emptyLead')}</p>
      <button type="button" class="btn primary" disabled={creating} onclick={newPlaylist}>
        <Icon name="plus" size={16} />
        {t('playlists.new')}
      </button>
    </div>
  {:else}
    <ul class="grid">
      {#each list as playlist (playlist.id)}
        <li class="card">
          <button type="button" class="open" onclick={() => go('playlist', String(playlist.id))}>
            <span class="art">
              <PlaylistCover coverIds={playlist.coverIds} seed={playlist.id} fluid radius="12px" />
            </span>
            <span class="name">{playlist.name}</span>
            <span class="meta">
              {tc('view.count.songs', playlist.trackCount)}{#if playlist.durationMs > 0}, {length(playlist.durationMs)}{/if}
            </span>
          </button>
          {#if playlist.trackCount > 0}
            <button
              type="button"
              class="play"
              aria-label={t('album.playTitle', { title: playlist.name })}
              title={t('album.playTitle', { title: playlist.name })}
              onclick={() => playPlaylist(playlist.id)}
            >
              <Icon name="play" size={18} filled />
            </button>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if list !== null && remote.length}
    <section class="remote" aria-labelledby="pl-remote-head">
      <header class="section-head">
        <h2 id="pl-remote-head">
          <!-- The Navidrome source dot; unified with the shared source badge at merge. -->
          <span class="source-dot" data-source="navidrome"></span>
          {t('source.navidrome')}
        </h2>
        <p class="section-lead">{t('playlists.remote.lead')}</p>
      </header>
      <ul class="grid">
        {#each remote as playlist (playlist.id)}
          <li class="card">
            <button type="button" class="open" onclick={() => go('playlist', playlist.id)}>
              <span class="art">
                <PlaylistCover coverIds={playlist.coverIds} seed={remoteSeed(playlist.id)} fluid radius="12px" />
              </span>
              <span class="name">{playlist.name}</span>
              <span class="meta">
                <span class="readonly" title={t('playlist.readOnlyHint')}>
                  <svg
                    width="12"
                    height="12"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.8"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    aria-hidden="true"
                  >
                    <rect x="5.5" y="10.5" width="13" height="9" rx="2" />
                    <path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5" />
                  </svg>
                  {t('playlist.readOnly')}
                </span>
                <span class="count">{tc('view.count.songs', playlist.trackCount)}</span>
              </span>
            </button>
            {#if playlist.trackCount > 0}
              <button
                type="button"
                class="play"
                aria-label={t('album.playTitle', { title: playlist.name })}
                title={t('album.playTitle', { title: playlist.name })}
                onclick={() => playRemotePlaylist(playlist.id)}
              >
                <Icon name="play" size={18} filled />
              </button>
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {/if}
</div>

<style>
  .scroll {
    scrollbar-color: auto;
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 4px 32px 40px;
    box-sizing: border-box;
  }

  .grid {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 22px 24px;
  }

  .card {
    position: relative;
    min-width: 0;
  }

  .open {
    display: flex;
    flex-direction: column;
    width: 100%;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
    border-radius: 14px;
  }

  .open:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 4px;
  }

  .art {
    display: block;
    width: 100%;
    border-radius: 12px;
    box-shadow: 0 10px 24px -14px rgba(0, 0, 0, 0.9);
    transition: filter var(--dur) ease-out;
  }

  .card:hover .art {
    filter: brightness(1.08);
  }

  .name {
    margin-top: 11px;
    font-size: 13.5px;
    font-weight: 560;
    line-height: 18px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .meta {
    margin-top: 2px;
    font-size: 12.5px;
    line-height: 17px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  /* The same round play button as on album cards, on the art's lower right. */
  .play {
    position: absolute;
    right: 10px;
    /* Above the caption (11 + 18 + 2 + 17 px), 10 px up from the art's bottom edge. */
    bottom: 58px;
    width: 42px;
    height: 42px;
    display: grid;
    place-items: center;
    padding: 0 0 0 2px;
    border: 0;
    border-radius: 50%;
    background: var(--text);
    color: var(--bg);
    box-shadow: 0 8px 20px -6px rgba(0, 0, 0, 0.8);
    cursor: pointer;
    opacity: 0;
    transform: translateY(4px);
    transition:
      opacity var(--dur) ease-out,
      transform var(--dur) ease-out,
      background-color var(--dur) ease-out;
  }

  .card:hover .play,
  .play:focus-visible {
    opacity: 1;
    transform: none;
  }

  .play:hover {
    background: #fff;
  }

  .play:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
  }

  /* The Navidrome section: a hairline above, the source in its color, then the same cards. */
  .remote {
    margin-top: 40px;
    padding-top: 26px;
    border-top: 1px solid var(--line);
  }

  .section-head {
    margin-bottom: 20px;
  }

  .section-head h2 {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 0 0 4px;
  }

  .section-lead {
    margin: 0;
    font-size: 13px;
    color: var(--text-dim);
  }

  .source-dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--src-navidrome);
    box-shadow: 0 0 10px color-mix(in srgb, var(--src-navidrome) 55%, transparent);
  }

  .remote .meta {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .readonly {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: color-mix(in srgb, var(--src-navidrome) 70%, var(--text-dim));
  }

  .count {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* No playlists: three sheets fanned out, an invitation to start the first. */
  .empty {
    max-width: 440px;
    padding: 20px 0 12px;
  }

  .stack {
    position: relative;
    width: 92px;
    height: 84px;
    margin-bottom: 22px;
  }

  .sheet {
    position: absolute;
    left: 14px;
    top: 6px;
    width: 64px;
    height: 64px;
    border-radius: 12px;
    background: linear-gradient(145deg, #2a2a31, #1a1a1f);
    box-shadow:
      inset 0 0 0 1px rgba(255, 255, 255, 0.07),
      0 12px 24px -12px rgba(0, 0, 0, 0.9);
  }

  .s1 {
    transform: rotate(-12deg) translate(-10px, 4px);
    background: linear-gradient(145deg, color-mix(in srgb, var(--spec-4) 22%, #1f1f25), #18181d);
  }

  .s2 {
    transform: rotate(9deg) translate(12px, 2px);
    background: linear-gradient(145deg, color-mix(in srgb, var(--spec-3) 18%, #1f1f25), #18181d);
  }

  .s3 {
    display: grid;
    place-items: center;
    color: var(--text);
    background: linear-gradient(145deg, color-mix(in srgb, var(--spec-1) 24%, #26262d), #1b1b20);
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
