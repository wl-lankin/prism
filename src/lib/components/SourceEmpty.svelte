<script lang="ts">
  import { lang, t, type Key } from '../i18n';
  import { navidromeSync } from '../ipc';
  import { go } from '../stores/nav.svelte';
  import { failureText, navidrome, navidromeErrorText } from '../stores/navidrome.svelte';
  import type { Source } from '../types';
  import EmptyLibrary from './EmptyLibrary.svelte';

  /** A source view with nothing in the index yet. Local files keep the add-a-folder screen;
   *  Navidrome says why it is empty: not connected, the first sync running (with its
   *  progress), not reachable, a failure that needs the user, or a server without music. */
  let {
    source,
    offerLocal = false,
  }: {
    source: Source;
    /** Shown in the views of every source: also offer the way to local music. */
    offerLocal?: boolean;
  } = $props();

  type Stage = 'connect' | 'syncing' | 'offline' | 'error' | 'empty';

  const stage: Stage = $derived.by(() => {
    if (!navidrome.configured || navidrome.state === 'off') return 'connect';
    if (navidrome.state === 'syncing') return 'syncing';
    if (navidrome.state === 'offline') return 'offline';
    if (navidrome.state === 'error') return 'error';
    return 'empty';
  });

  const titles: Record<Stage, Key> = {
    connect: 'sourceEmpty.connectTitle',
    syncing: 'sourceEmpty.syncTitle',
    offline: 'sourceEmpty.offlineTitle',
    error: 'sourceEmpty.errorTitle',
    empty: 'sourceEmpty.emptyTitle',
  };

  const lead = $derived.by(() => {
    if (stage === 'connect') return t('sourceEmpty.connectLead');
    if (stage === 'syncing') return t('sourceEmpty.syncLead');
    if (stage === 'offline') return t('sourceEmpty.offlineLead');
    if (stage === 'error') return navidrome.error ? failureText(navidrome.error) : t('navidrome.error.internal');
    return t('sourceEmpty.emptyLead');
  });

  const progress = $derived(navidrome.progress);
  const share = $derived(progress?.total ? Math.min(1, progress.done / progress.total) : null);
  const progressText = $derived.by(() => {
    const fmt = (n: number) => n.toLocaleString(lang());
    if (!progress) return t('sourceEmpty.syncStarting');
    if (progress.total) return t('sourceEmpty.syncProgress', { done: fmt(progress.done), total: fmt(progress.total) });
    return t('sourceEmpty.syncCount', { done: fmt(progress.done) });
  });

  let asking = $state(false);
  let failed = $state<string | null>(null);

  async function syncNow() {
    asking = true;
    failed = null;
    try {
      await navidromeSync();
    } catch (err) {
      failed = navidromeErrorText(err);
    } finally {
      asking = false;
    }
  }
</script>

{#if source === 'local'}
  <EmptyLibrary />
{:else}
  <div class="empty" data-stage={stage}>
    <div class="panel">
      <!-- The source's own dot, grown into a quiet emblem: hollow while the server is away. -->
      <div class="emblem" class:hollow={stage === 'offline' || stage === 'error'} class:dim={stage === 'connect'} aria-hidden="true">
        <span class="halo"></span>
        <span class="core"></span>
      </div>
      <h1>{t(titles[stage])}</h1>
      <!-- Screen readers hear the state when it changes, not every progress number. -->
      <span class="sr-only" role="status" aria-live="polite">{t(titles[stage])}</span>
      <p class="lead">{lead}</p>

      {#if stage === 'connect' || stage === 'error'}
        <button type="button" class="btn primary" onclick={() => go('settings')}>{t('sourceEmpty.openSettings')}</button>
      {:else if stage === 'empty'}
        <button type="button" class="btn ghost" disabled={asking} onclick={syncNow}>{t('sourceEmpty.syncNow')}</button>
      {/if}

      {#if stage === 'syncing'}
        <div class="progress">
          <div class="bar" class:indeterminate={share === null}>
            <i style={share !== null ? `transform:scaleX(${share})` : undefined}></i>
          </div>
          <span class="label">{progressText}</span>
        </div>
      {/if}

      {#if failed}<p class="error" role="alert">{failed}</p>{/if}

      {#if offerLocal}
        <button type="button" class="local" onclick={() => go('albums', undefined, 'local')}>
          {t('sourceEmpty.addFolder')}
        </button>
      {/if}
    </div>
  </div>
{/if}

<style>
  .empty {
    height: 100%;
    display: grid;
    place-items: center;
    padding: 0 32px 48px;
    box-sizing: border-box;
    animation: fade var(--dur) ease-out;
  }

  .panel {
    width: min(440px, 100%);
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
  }

  .emblem {
    --c: var(--src-navidrome);
    position: relative;
    width: 96px;
    height: 96px;
    display: grid;
    place-items: center;
    margin-bottom: 28px;
  }

  .halo {
    position: absolute;
    inset: 0;
    border-radius: 50%;
    background: radial-gradient(circle, color-mix(in srgb, var(--c) 22%, transparent) 0%, transparent 68%);
    transition: opacity var(--dur) ease-out;
  }

  .core {
    width: 26px;
    height: 26px;
    border-radius: 50%;
    background: var(--c);
    box-shadow:
      0 0 0 6px color-mix(in srgb, var(--c) 10%, transparent),
      0 0 26px color-mix(in srgb, var(--c) 55%, transparent);
    transition:
      background-color var(--dur) ease-out,
      box-shadow var(--dur) ease-out;
  }

  .emblem.hollow .core {
    background: transparent;
    box-shadow: inset 0 0 0 2.5px var(--c);
  }

  .emblem.hollow .halo,
  .emblem.dim .halo {
    opacity: 0.35;
  }

  .emblem.dim .core {
    opacity: 0.55;
    box-shadow: none;
  }

  h1 {
    margin: 0 0 10px;
    font-family: var(--font-display);
    font-size: 26px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }

  .lead {
    margin: 0 0 26px;
    font-size: 14px;
    line-height: 1.6;
    color: var(--text-dim);
    text-wrap: pretty;
  }

  .progress {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }

  .bar {
    position: relative;
    width: 260px;
    height: 3px;
    border-radius: 2px;
    overflow: hidden;
    background: rgba(255, 255, 255, 0.08);
  }

  .bar i {
    position: absolute;
    inset: 0;
    transform-origin: left;
    background: var(--src-navidrome);
    box-shadow: 0 0 10px color-mix(in srgb, var(--src-navidrome) 50%, transparent);
    transition: transform var(--dur) linear;
  }

  /* Without a total the bar is a quiet full-width line; the sweep (an endless loop) runs only
     where motion is welcome. */
  .bar.indeterminate i {
    width: 100%;
    opacity: 0.5;
  }

  @media (prefers-reduced-motion: no-preference) {
    .bar.indeterminate i {
      width: 40%;
      opacity: 1;
      animation: sweep 1.2s ease-in-out infinite;
    }
  }

  .label {
    font-size: 12.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  /* A quiet text link under the main action: the way to local files. */
  .local {
    margin-top: 22px;
    padding: 4px 6px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--text-dim);
    font: inherit;
    font-size: 13px;
    text-decoration: underline;
    text-underline-offset: 3px;
    text-decoration-color: rgba(243, 243, 245, 0.3);
    cursor: pointer;
    transition:
      color var(--dur) ease-out,
      text-decoration-color var(--dur) ease-out;
  }

  .local:hover {
    color: var(--text);
    text-decoration-color: rgba(243, 243, 245, 0.6);
  }

  .local:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
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

  .error {
    margin: 14px 0 0;
    font-size: 13px;
    color: var(--spec-1);
  }

  @keyframes sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(260%);
    }
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }
</style>
