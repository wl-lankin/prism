<script lang="ts">
  import type { Snippet } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { t } from '../i18n';
  import { windowState } from '../stores/window.svelte';
  import Icon from './Icon.svelte';

  /** The drag strip at the top of the content column, with min / max / close on the right.
   *  `children` renders on the left (view title, search) and stays clickable. */
  let { children }: { children?: Snippet } = $props();

  /** Followed by the shared window store, which `App` starts. */
  const maximized = $derived(windowState.maximized);

  function act(action: 'minimize' | 'toggleMaximize' | 'close') {
    try {
      getCurrentWindow()[action]().catch((err: unknown) => console.error(`window ${action} failed`, err));
    } catch (err) {
      console.error(`window ${action} failed`, err);
    }
  }
</script>

<header class="titlebar" data-tauri-drag-region>
  <div class="lead" data-tauri-drag-region>
    {@render children?.()}
  </div>
  <div class="controls">
    <button type="button" class="ctl" aria-label={t('window.minimize')} title={t('window.minimize')} onclick={() => act('minimize')}>
      <Icon name="minimize" size={16} />
    </button>
    <button
      type="button"
      class="ctl"
      aria-label={maximized ? t('window.restore') : t('window.maximize')}
      title={maximized ? t('window.restore') : t('window.maximize')}
      onclick={() => act('toggleMaximize')}
    >
      <Icon name={maximized ? 'restore' : 'maximize'} size={16} />
    </button>
    <button type="button" class="ctl close" aria-label={t('window.close')} title={t('window.close')} onclick={() => act('close')}>
      <Icon name="close" size={16} />
    </button>
  </div>
</header>

<style>
  .titlebar {
    display: flex;
    align-items: center;
    gap: 16px;
    height: var(--header-h);
    padding: 0 10px 0 32px;
    user-select: none;
    -webkit-user-select: none;
  }

  .lead {
    flex: 1;
    min-width: 0;
    height: 100%;
    display: flex;
    align-items: center;
  }

  .controls {
    display: flex;
    gap: 2px;
  }

  .ctl {
    width: 38px;
    height: 30px;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: var(--radius-s);
    background: transparent;
    color: var(--text-dim);
    cursor: default;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .ctl:hover {
    background: rgba(255, 255, 255, 0.07);
    color: var(--text);
  }

  .ctl:active {
    background: rgba(255, 255, 255, 0.04);
  }

  .ctl.close:hover {
    background: rgba(255, 77, 109, 0.9);
    color: #fff;
  }

  .ctl:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: -2px;
  }
</style>
