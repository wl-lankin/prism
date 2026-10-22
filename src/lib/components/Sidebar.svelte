<script lang="ts">
  import logoUrl from '../../../assets/logo.svg';
  import { t, type Key } from '../i18n';
  import { go, nav, overlays, type View } from '../stores/nav.svelte';
  import { navidrome } from '../stores/navidrome.svelte';
  import type { Source } from '../types';
  import Icon, { type IconName } from './Icon.svelte';

  interface Item {
    label: Key;
    icon?: IconName;
    dot?: string;
    view?: View;
    /** The view's parameter, e.g. the radio tab. */
    param?: string;
    /** Views that keep this item highlighted (while no source is chosen). */
    match?: View[];
    /** A source entry: opens the source's albums, and is lit wherever that source is. */
    source?: Source;
    soon?: boolean;
  }

  const top: Item[] = [
    { label: 'nav.home', icon: 'home', view: 'home', match: ['home'] },
    { label: 'nav.favorites', icon: 'heart', view: 'favorites', match: ['favorites'] },
    { label: 'nav.playlists', icon: 'playlists', view: 'playlists', match: ['playlists', 'playlist'] },
  ];

  const library: Item[] = [
    { label: 'nav.albums', icon: 'album', view: 'albums', match: ['albums', 'album'] },
    { label: 'nav.artists', icon: 'artist', view: 'artists', match: ['artists', 'artist'] },
    { label: 'nav.songs', icon: 'songs', view: 'songs', match: ['songs'] },
  ];

  // No YouTube entry until it lands (owner decision).
  const sources: Item[] = [
    { label: 'source.local', dot: 'var(--src-local)', view: 'albums', source: 'local' },
    { label: 'source.navidrome', dot: 'var(--src-navidrome)', view: 'albums', source: 'navidrome' },
    { label: 'source.radio', dot: 'var(--src-radio)', view: 'radio', param: 'near', match: ['radio'] },
  ];

  /** The Navidrome dot: hollow while the server is away or the sync needs the user, faint
   *  while no account is signed in, breathing while a sync runs. The state is in the name. */
  const link = $derived.by(() => {
    if (!navidrome.configured || navidrome.state === 'off') return { dot: 'dim', label: 'source.navidromeNotConnected' as Key };
    if (navidrome.state === 'offline') return { dot: 'hollow', label: 'source.navidromeOffline' as Key };
    if (navidrome.state === 'error') return { dot: 'hollow', label: 'source.navidromeFailed' as Key };
    return { dot: navidrome.state === 'syncing' ? 'syncing' : '', label: null };
  });

  function isActive(item: Item): boolean {
    if (item.source) return nav.source === item.source;
    return !nav.source && !!item.match?.includes(nav.view);
  }

  /** The "Coming soon" pill floats beside the hovered or keyboard-focused disabled entry. It
   *  lives outside the scrolling list, whose overflow and fade mask would cut it off. */
  let pill = $state({ shown: false, delayed: false, x: 0, y: 0 });

  function showPill(event: Event, delayed: boolean) {
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    pill = { shown: true, delayed, x: box.right + 8, y: box.top + box.height / 2 };
  }

  function hidePill() {
    pill.shown = false;
  }
</script>

{#snippet entry(item: Item)}
  {@const active = isActive(item)}
  {@const linked = item.source === 'navidrome' ? link : null}
  <li>
    <button
      type="button"
      class="item"
      class:active
      class:soon={item.soon}
      aria-current={active ? 'page' : undefined}
      aria-disabled={item.soon ? 'true' : undefined}
      aria-label={linked?.label ? t(linked.label) : undefined}
      onclick={() => item.view && !item.soon && go(item.view, item.param, item.source)}
      onpointerenter={item.soon ? (event) => showPill(event, true) : undefined}
      onpointerleave={item.soon ? hidePill : undefined}
      onfocus={item.soon
        ? (event) => (event.currentTarget as HTMLElement).matches(':focus-visible') && showPill(event, false)
        : undefined}
      onblur={item.soon ? hidePill : undefined}
    >
      {#if item.icon}
        <Icon name={item.icon} size={18} />
      {:else}
        <span
          class="dot"
          class:hollow={linked?.dot === 'hollow'}
          class:dim={linked?.dot === 'dim'}
          class:syncing={linked?.dot === 'syncing'}
          style="--dot:{item.dot}"
        ></span>
      {/if}
      <span class="label">{t(item.label)}</span>
      {#if item.soon}
        <!-- Always in the accessible name; the visible pill is the floating .soon-tip. -->
        <span class="sr-only">{t('nav.comingSoon')}</span>
      {/if}
    </button>
  </li>
{/snippet}

<nav class="sidebar" aria-label={t('app.name')}>
  <div class="brand-row" data-tauri-drag-region>
    <button type="button" class="brand" aria-label={t('app.about')} title={t('app.about')} onclick={() => (overlays.about = true)}>
      <img src={logoUrl} alt="" width="26" height="26" draggable="false" />
      <span class="wordmark">Prism</span>
    </button>
  </div>

  <div class="scroll" onscroll={hidePill}>
    <ul>
      {#each top as item (item.label)}{@render entry(item)}{/each}
    </ul>

    <h2>{t('nav.library')}</h2>
    <ul>
      {#each library as item (item.label)}{@render entry(item)}{/each}
    </ul>

    <h2>{t('nav.sources')}</h2>
    <ul>
      {#each sources as item (item.label)}{@render entry(item)}{/each}
    </ul>
  </div>

  <ul class="foot">
    {@render entry({ label: 'nav.settings', icon: 'settings', view: 'settings', match: ['settings'] })}
  </ul>

  <span
    class="soon-tip"
    class:shown={pill.shown}
    class:delayed={pill.delayed}
    style="left:{pill.x}px; top:{pill.y}px"
    aria-hidden="true">{t('nav.comingSoon')}</span
  >
</nav>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    min-height: 0;
    padding: 0 12px 14px;
    background: rgba(0, 0, 0, 0.16);
    user-select: none;
    -webkit-user-select: none;
  }

  .brand-row {
    height: var(--header-h);
    display: flex;
    align-items: center;
    padding-left: 4px;
    margin-bottom: 10px;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 10px 5px 5px;
    margin-left: -1px;
    border: 0;
    border-radius: 10px;
    background: transparent;
    color: var(--text);
    cursor: pointer;
    transition: background-color var(--dur) ease-out;
  }

  .brand:hover {
    background: rgba(255, 255, 255, 0.05);
  }

  .brand img {
    border-radius: 7px;
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.08),
      0 4px 12px rgba(0, 0, 0, 0.4);
  }

  .wordmark {
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  /* Scrolls when the window is short (600 px minimum), fading out above Settings. */
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    margin: 0 -6px;
    /* The bar's room is always kept (the gutter stands in for the right padding), so a
       list that overflows for a moment, as it can while a view swaps in, never squeezes
       the entries (owner, 2026-10-22). */
    padding: 0 0 20px 6px;
    scrollbar-gutter: stable;
    mask-image: linear-gradient(to bottom, #000 calc(100% - 22px), transparent);
    /* The inherited standard scrollbar-color would override the slim bar below. */
    scrollbar-color: auto;
  }

  /* A slim bar in the gutter beside the entries (no arrow buttons), clearer on hover. */
  .scroll::-webkit-scrollbar {
    width: 6px;
  }

  .scroll::-webkit-scrollbar-thumb {
    border: 0;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.1);
  }

  .scroll:hover::-webkit-scrollbar-thumb {
    background: rgba(255, 255, 255, 0.22);
  }

  h2 {
    margin: 22px 12px 6px;
    font-size: 11.5px;
    font-weight: 500;
    letter-spacing: 0.01em;
    color: rgba(243, 243, 245, 0.4);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    /* Without it the column grows to the entries' unwrapped width and overflows the list. */
    grid-template-columns: minmax(0, 1fr);
    gap: 1px;
  }

  li {
    position: relative;
  }

  .item {
    position: relative;
    width: 100%;
    height: 34px;
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: rgba(243, 243, 245, 0.72);
    font: inherit;
    font-size: 13.5px;
    font-weight: 450;
    text-align: left;
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .item:hover:not(.soon) {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text);
  }

  .item.active {
    background: rgba(255, 255, 255, 0.085);
    color: var(--text);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.05);
  }

  .item.soon {
    color: rgba(243, 243, 245, 0.34);
    cursor: default;
  }

  .item:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: -2px;
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dot {
    width: 18px;
    height: 18px;
    display: grid;
    place-items: center;
    flex: none;
  }

  .dot::before {
    content: '';
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--dot);
    box-shadow: 0 0 10px color-mix(in srgb, var(--dot) 55%, transparent);
  }

  .soon .dot::before,
  .dot.dim::before {
    opacity: 0.45;
    box-shadow: none;
  }

  .dot::before {
    transition:
      background-color var(--dur) ease-out,
      box-shadow var(--dur) ease-out,
      opacity var(--dur) ease-out;
  }

  /* Offline or failed: the ring stays, the light goes out. */
  .dot.hollow::before {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--dot);
  }

  /* A sync is running: the glow breathes. An endless loop, so only where motion is welcome;
     under reduced motion the dot stays still at a calm mid opacity. */
  .dot.syncing::before {
    opacity: 0.7;
  }

  @media (prefers-reduced-motion: no-preference) {
    .dot.syncing::before {
      opacity: 1;
      animation: breathe 1.8s ease-in-out infinite;
    }
  }

  @keyframes breathe {
    50% {
      box-shadow: 0 0 3px color-mix(in srgb, var(--dot) 25%, transparent);
      opacity: 0.7;
    }
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

  /* "Coming soon": a pill right beside a disabled entry, over the main area. Fixed, outside
     the scrolling list, so neither its overflow nor its fade mask cuts the label off. */
  .soon-tip {
    position: fixed;
    z-index: 4;
    padding: 3px 7px;
    border-radius: 6px;
    background: var(--raised);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
    color: var(--text);
    font-size: 11px;
    white-space: nowrap;
    pointer-events: none;
    opacity: 0;
    transform: translate(-4px, -50%);
    transition:
      opacity var(--dur) ease-out,
      transform var(--dur) ease-out;
  }

  .soon-tip.shown {
    opacity: 1;
    transform: translate(0, -50%);
  }

  .soon-tip.shown.delayed {
    transition-delay: 250ms;
  }

  .foot {
    padding-top: 10px;
  }
</style>
