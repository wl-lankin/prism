<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { lang, t } from '../i18n';
  import { libraryAddFolder } from '../ipc';
  import { library, reloadLibrary } from '../stores/library.svelte';
  import Icon from './Icon.svelte';
  import PrismIllustration from './PrismIllustration.svelte';

  /** Shown while the library has no tracks: one action, adding a music folder, and the scan
   *  progress right under it. `compact`: a left-aligned section under other Home content
   *  (favorite stations), without the illustration. */
  let { compact = false }: { compact?: boolean } = $props();
  let adding = $state(false);
  let error = $state<string | null>(null);

  const scan = $derived(library.scan);
  const busy = $derived(adding || !!scan);
  const share = $derived(scan && scan.total > 0 ? Math.min(1, scan.done / scan.total) : 0);

  async function add() {
    error = null;
    let path: string | string[] | null;
    try {
      path = await open({ directory: true, multiple: false, title: t('library.chooseFolder') });
    } catch (err) {
      error = t('library.addFailed', { message: String(err) });
      return;
    }
    if (typeof path !== 'string') return;
    adding = true;
    try {
      await libraryAddFolder(path);
      await reloadLibrary();
    } catch (err) {
      error = t('library.addFailed', { message: String(err) });
    } finally {
      adding = false;
    }
  }
</script>

<div class="empty" class:compact>
  <div class="panel">
    {#if compact}
      <h2>{t('library.emptyTitle')}</h2>
    {:else}
      <div class="mark">
        <PrismIllustration width={150} />
      </div>
      <h1>{t('library.emptyTitle')}</h1>
    {/if}
    <p class="lead">{t('library.emptyLead')}</p>

    <button type="button" class="btn primary" disabled={busy} onclick={add}>
      <Icon name="folderAdd" size={18} />
      {t('library.addFolder')}
    </button>

    <div class="progress" class:shown={busy} role="status" aria-live="polite">
      {#if busy}
        <div class="bar" class:indeterminate={!scan || scan.total === 0}>
          <i style={scan && scan.total > 0 ? `transform:scaleX(${share})` : undefined}></i>
        </div>
        <span class="label">
          {scan && scan.total > 0
            ? t('library.scanning', { done: scan.done.toLocaleString(lang()), total: scan.total.toLocaleString(lang()) })
            : t('library.looking')}
        </span>
      {/if}
    </div>

    {#if error}<p class="error" role="alert">{error}</p>{/if}
  </div>
</div>

<style>
  .empty {
    height: 100%;
    display: grid;
    place-items: center;
    padding: 0 32px 48px;
    box-sizing: border-box;
  }

  .panel {
    width: min(440px, 100%);
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
  }

  /* The logo sits on a soft pool of the spectrum, the one bit of color on this screen. */
  .mark {
    margin-bottom: 24px;
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
    min-height: 44px;
    margin-top: 22px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    opacity: 0;
    transition: opacity var(--dur) ease-out;
  }

  .progress.shown {
    opacity: 1;
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
    background: linear-gradient(90deg, var(--spec-1), var(--spec-2), var(--spec-3), var(--spec-4));
    transition: transform var(--dur) linear;
  }

  .bar.indeterminate i {
    width: 40%;
    animation: sweep 1.2s ease-in-out infinite;
  }

  .label {
    font-size: 12.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .error {
    margin: 12px 0 0;
    font-size: 13px;
    color: var(--spec-1);
  }

  .compact {
    height: auto;
    display: block;
    padding: 0 32px 32px;
  }

  .compact .panel {
    width: min(520px, 100%);
    align-items: flex-start;
    text-align: left;
  }

  .compact h2 {
    margin: 0 0 6px;
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .compact .lead {
    margin-bottom: 16px;
  }

  .compact .progress {
    align-items: flex-start;
  }

  @keyframes sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(260%);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .bar.indeterminate i {
      animation: none;
      width: 100%;
      opacity: 0.5;
    }
  }
</style>
