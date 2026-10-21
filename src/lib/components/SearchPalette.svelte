<script lang="ts">
  import { flushSync, tick } from 'svelte';
  import { formatDuration } from '../format';
  import { t, tc, type Key } from '../i18n';
  import { librarySearch } from '../ipc';
  import { playAlbum } from '../play';
  import { hasMixedSources } from '../stores/library.svelte';
  import { go } from '../stores/nav.svelte';
  import type { Album, Artist, SearchResult, Track } from '../types';
  import Cover from './Cover.svelte';
  import Icon from './Icon.svelte';
  import SourceBadge from './SourceBadge.svelte';

  /** Ctrl+K / Cmd+K: search the whole library. Results come in three groups; arrows move
   *  through all of them, Enter opens an album or artist or plays a song with its album. */
  let { open = $bindable(false) }: { open?: boolean } = $props();

  /** Results carry their source's dot once the library has more than one source. */
  const mixed = $derived(hasMixedSources());

  const DEBOUNCE_MS = 120;
  const LIMIT = 8;

  type Item =
    | { kind: 'album'; album: Album }
    | { kind: 'artist'; artist: Artist }
    | { kind: 'song'; track: Track };

  let query = $state('');
  let result = $state.raw<SearchResult | null>(null);
  let error = $state<string | null>(null);
  let active = $state(0);
  let input = $state<HTMLInputElement>();
  let list = $state<HTMLDivElement>();
  let returnFocus: HTMLElement | null = null;

  const groups = $derived.by(() => {
    const out: { label: Key; items: Item[] }[] = [];
    if (!result) return out;
    if (result.albums.length) out.push({ label: 'nav.albums', items: result.albums.map((album) => ({ kind: 'album', album })) });
    if (result.artists.length) out.push({ label: 'nav.artists', items: result.artists.map((artist) => ({ kind: 'artist', artist })) });
    if (result.tracks.length) out.push({ label: 'nav.songs', items: result.tracks.map((track) => ({ kind: 'song', track })) });
    return out;
  });
  const items = $derived(groups.flatMap((group) => group.items));
  /** Index of each group's first item in `items`. */
  const offsets = $derived(groups.map((_, g) => groups.slice(0, g).reduce((n, group) => n + group.items.length, 0)));

  /** The query the shown result belongs to. */
  let resultFor = $state('');
  let timer: ReturnType<typeof setTimeout> | undefined;

  // Each keystroke restarts the timer; only the latest request may set the result.
  let seq = 0;

  function search(q: string, mine: number): Promise<void> {
    return librarySearch(q, LIMIT)
      .then((found) => {
        if (mine !== seq) return;
        result = found ?? { albums: [], artists: [], tracks: [] };
        resultFor = q;
        error = null;
        active = 0;
      })
      .catch((err) => {
        if (mine === seq) error = t('search.failed', { message: String(err) });
      });
  }

  $effect(() => {
    const q = query.trim();
    const mine = ++seq;
    if (!q) {
      result = null;
      resultFor = '';
      error = null;
      return;
    }
    timer = setTimeout(() => void search(q, mine), DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });

  /** Enter: acts on the result of what is typed now. Inside the debounce window the search
   *  runs at once and Enter takes its first entry; it never acts on an older query's result. */
  async function enter() {
    // Let the debounce effect see the latest keystroke first, so its timer is the one cleared.
    flushSync();
    const q = query.trim();
    if (!q) return;
    if (q !== resultFor) {
      clearTimeout(timer);
      const mine = ++seq;
      await search(q, mine);
      if (mine !== seq || resultFor !== q) return;
    }
    const item = items[active];
    if (item) choose(item);
  }

  $effect(() => {
    if (!open) return;
    returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    void tick().then(() => input?.select());
    return () => returnFocus?.focus?.();
  });

  function close() {
    open = false;
  }

  function choose(item: Item) {
    close();
    if (item.kind === 'album') go('album', item.album.id);
    else if (item.kind === 'artist') go('artist', item.artist.name);
    else void playAlbum(item.track.albumId, { trackId: item.track.id });
  }

  function move(to: number) {
    if (!items.length) return;
    active = (to + items.length) % items.length;
    void tick().then(() => list?.querySelector(`#search-${active}`)?.scrollIntoView({ block: 'nearest' }));
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      close();
    } else if (event.key === 'ArrowDown') {
      event.preventDefault();
      move(active + 1);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      move(active - 1);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      void enter();
    } else if (event.key === 'Tab') {
      // Focus stays in the field; Tab moves through the results like the arrows.
      event.preventDefault();
      move(active + (event.shiftKey ? -1 : 1));
    }
  }
</script>

<svelte:window
  onkeydown={(event) => {
    // Esc closes wherever focus is (the field handles its own keys first).
    if (open && event.key === 'Escape' && event.target !== input) {
      event.preventDefault();
      close();
    }
  }}
/>

{#if open}
  <div class="overlay" role="presentation" onmousedown={(e) => e.target === e.currentTarget && close()}>
    <!-- Mouse presses on the palette never take focus from the field. -->
    <div
      class="palette"
      role="dialog"
      aria-modal="true"
      aria-label={t('search.label')}
      tabindex="-1"
      onmousedown={(e) => e.target !== input && e.preventDefault()}
    >
      <div class="field">
        <Icon name="search" size={20} />
        <input
          bind:this={input}
          bind:value={query}
          type="text"
          role="combobox"
          aria-expanded={items.length > 0}
          aria-controls="search-results"
          aria-activedescendant={items.length ? `search-${active}` : undefined}
          aria-autocomplete="list"
          aria-label={t('search.label')}
          placeholder={t('search.placeholder')}
          spellcheck="false"
          autocomplete="off"
          {onkeydown}
        />
        <kbd>Esc</kbd>
      </div>

      <div class="results" id="search-results" role="listbox" aria-label={t('search.label')} bind:this={list}>
        {#if error}
          <p class="note">{error}</p>
        {:else if !query.trim()}
          <p class="note">{t('search.hint')}</p>
        {:else if result && items.length === 0}
          <p class="note">{t('search.empty', { query: query.trim() })}</p>
        {/if}

        {#each groups as group, g (group.label)}
          <div class="group" role="group" aria-labelledby="search-group-{g}">
            <h2 id="search-group-{g}">{t(group.label)}</h2>
            {#each group.items as item, i (i)}
              {@const index = offsets[g] + i}
              <div
                id="search-{index}"
                class="item"
                class:active={index === active}
                role="option"
                aria-selected={index === active}
                tabindex="-1"
                onmousemove={() => (active = index)}
                onclick={() => choose(item)}
                onkeydown={onkeydown}
              >
                {#if item.kind === 'album'}
                  <span class="art">
                    <Cover coverId={item.album.coverId} size={40} cache={96} radius="6px" />
                    {#if mixed && item.album.source !== 'local'}<SourceBadge source={item.album.source} size={9} />{/if}
                  </span>
                  <span class="text">
                    <span class="main">{item.album.title}</span>
                    <span class="sub">{item.album.artist}{item.album.year ? `, ${item.album.year}` : ''}</span>
                  </span>
                {:else if item.kind === 'artist'}
                  <span class="art">
                    <Cover coverId={item.artist.coverId} size={40} cache={96} radius="50%" />
                    {#if mixed && item.artist.source !== 'local'}<SourceBadge source={item.artist.source} size={9} />{/if}
                  </span>
                  <span class="text">
                    <span class="main">{item.artist.name}</span>
                    <span class="sub">{tc('view.count.albums', item.artist.albumCount)}</span>
                  </span>
                {:else}
                  <span class="art">
                    <Cover coverId={item.track.coverId} size={40} cache={96} radius="6px" />
                    {#if mixed && item.track.source !== 'local'}<SourceBadge source={item.track.source} size={9} />{/if}
                  </span>
                  <span class="text">
                    <span class="main">{item.track.title}</span>
                    <span class="sub">{item.track.artist}, {item.track.album}</span>
                  </span>
                  <span class="time">{formatDuration(item.track.durationMs)}</span>
                {/if}
                {#if index === active}
                  <span class="enter" aria-hidden="true">
                    <Icon name={item.kind === 'song' ? 'play' : 'back'} size={14} filled={item.kind === 'song'} />
                  </span>
                {/if}
              </div>
            {/each}
          </div>
        {/each}
      </div>

      <footer>{t('search.keys')}</footer>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
    background: rgba(6, 6, 9, 0.55);
    backdrop-filter: blur(3px);
    animation: fade var(--dur) ease-out;
  }

  .palette {
    width: min(640px, calc(100vw - 64px));
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    border-radius: 16px;
    background: rgba(28, 28, 34, 0.97);
    box-shadow:
      inset 0 0 0 1px rgba(255, 255, 255, 0.09),
      0 40px 90px -20px rgba(0, 0, 0, 0.85),
      0 12px 30px rgba(0, 0, 0, 0.45);
    overflow: hidden;
    animation: rise var(--dur-l) cubic-bezier(0.2, 0.9, 0.3, 1);
  }

  /* A thin spectral edge under the field: the palette is Prism's own, not a system dialog. */
  .field {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
    flex: none;
    padding: 0 18px;
    height: 60px;
    color: var(--text-dim);
  }

  .field::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 1px;
    background: linear-gradient(90deg, var(--spec-1), var(--spec-2), var(--spec-3), var(--spec-4));
    opacity: 0.55;
  }

  input {
    flex: 1;
    min-width: 0;
    height: 100%;
    border: 0;
    outline: none;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 17px;
  }

  input::placeholder {
    color: rgba(243, 243, 245, 0.38);
  }

  kbd {
    padding: 2px 7px;
    border-radius: 6px;
    font: inherit;
    font-size: 11.5px;
    color: var(--text-dim);
    background: rgba(255, 255, 255, 0.06);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
  }

  .results {
    scrollbar-color: auto;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 6px 8px 8px;
  }

  .note {
    margin: 0;
    padding: 22px 12px;
    font-size: 13.5px;
    color: var(--text-dim);
  }

  h2 {
    margin: 10px 10px 4px;
    font-size: 12px;
    font-weight: 600;
    color: rgba(243, 243, 245, 0.45);
  }

  .item {
    display: flex;
    align-items: center;
    gap: 12px;
    height: 52px;
    padding: 0 10px;
    border-radius: 10px;
    cursor: pointer;
  }

  .item.active {
    background: rgba(255, 255, 255, 0.075);
  }

  .art {
    position: relative;
    flex: none;
    width: 40px;
    height: 40px;
  }

  .art :global(.source-badge) {
    position: absolute;
    right: -2px;
    bottom: -2px;
  }

  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .main,
  .sub {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .main {
    font-size: 14px;
    font-weight: 500;
  }

  .sub {
    font-size: 12.5px;
    color: var(--text-dim);
  }

  .time {
    font-size: 12.5px;
    color: rgba(243, 243, 245, 0.45);
    font-variant-numeric: tabular-nums;
  }

  .enter {
    width: 24px;
    height: 24px;
    display: grid;
    place-items: center;
    border-radius: 6px;
    color: var(--text);
    background: rgba(255, 255, 255, 0.08);
  }

  .enter :global(svg) {
    transform: scaleX(-1);
  }

  .enter :global(svg[fill='currentColor']) {
    transform: none;
  }

  footer {
    flex: none;
    padding: 10px 18px;
    font-size: 11.5px;
    color: rgba(243, 243, 245, 0.42);
    border-top: 1px solid var(--line);
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(-8px) scale(0.985);
    }
  }
</style>
