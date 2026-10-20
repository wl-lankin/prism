<script lang="ts">
  import logoUrl from '../../../assets/logo.svg';
  import { t, type Key } from '../i18n';
  import { go, nav, overlays, type View } from '../stores/nav.svelte';
  import Icon, { type IconName } from './Icon.svelte';

  interface Item {
    label: Key;
    icon?: IconName;
    dot?: string;
    view?: View;
    /** Views that keep this item highlighted. */
    match?: View[];
    soon?: boolean;
  }

  const top: Item[] = [
    { label: 'nav.home', icon: 'home', view: 'home', match: ['home'] },
    { label: 'nav.favorites', icon: 'heart', soon: true },
    { label: 'nav.playlists', icon: 'playlists', soon: true },
  ];

  const library: Item[] = [
    { label: 'nav.albums', icon: 'album', view: 'albums', match: ['albums', 'album'] },
    { label: 'nav.artists', icon: 'artist', view: 'artists', match: ['artists', 'artist'] },
    { label: 'nav.songs', icon: 'songs', view: 'songs', match: ['songs'] },
  ];

  const sources: Item[] = [
    { label: 'source.local', dot: 'var(--src-local)', view: 'albums' },
    { label: 'source.navidrome', dot: 'var(--src-navidrome)', soon: true },
    { label: 'source.youtube', dot: 'var(--src-youtube)', soon: true },
    { label: 'source.radio', dot: 'var(--src-radio)', soon: true },
  ];
</script>

{#snippet entry(item: Item)}
  {@const active = !!item.match?.includes(nav.view)}
  <li>
    <button
      type="button"
      class="item"
      class:active
      class:soon={item.soon}
      aria-current={active ? 'page' : undefined}
      aria-disabled={item.soon ? 'true' : undefined}
      data-tip={item.soon ? t('nav.comingSoon') : undefined}
      onclick={() => item.view && !item.soon && go(item.view)}
    >
      {#if item.icon}
        <Icon name={item.icon} size={18} />
      {:else}
        <span class="dot" style="--dot:{item.dot}"></span>
      {/if}
      <span class="label">{t(item.label)}</span>
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

  <div class="scroll">
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
    height: 52px;
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

  .scroll {
    flex: 1;
    min-height: 0;
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

  .soon .dot::before {
    opacity: 0.45;
    box-shadow: none;
  }

  /* "Coming soon", shown beside a disabled entry. */
  .item[data-tip]::after {
    content: attr(data-tip);
    position: absolute;
    left: calc(100% + 8px);
    top: 50%;
    z-index: 20;
    padding: 5px 9px;
    border-radius: 7px;
    background: var(--raised);
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.08),
      0 8px 24px rgba(0, 0, 0, 0.45);
    color: var(--text);
    font-size: 12px;
    white-space: nowrap;
    pointer-events: none;
    opacity: 0;
    transform: translate(-4px, -50%);
    transition:
      opacity var(--dur) ease-out,
      transform var(--dur) ease-out;
  }

  .item[data-tip]:hover::after {
    opacity: 1;
    transform: translate(0, -50%);
    transition-delay: 250ms;
  }

  .foot {
    padding-top: 10px;
  }
</style>
