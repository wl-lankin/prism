<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { t } from '../i18n';
  import { LYRICS_TOGGLE_ID } from '../queue';
  import { lyricsPanel, toggleLyricsPanel } from '../stores/queue.svelte';
  import { closeOverlay, openOverlay } from '../stores/overlays.svelte';
  import { ring } from '../stores/ring.svelte';
  import Icon from './Icon.svelte';
  import LyricsView from './LyricsView.svelte';

  /** The lyrics of the song that plays, in the sheet that slides in from the right (the
   *  queue panel's slot: opening one closes the other). The lyrics are only asked for while
   *  the sheet is open. */
  const open = $derived(lyricsPanel.open);

  // The sheet lies over the content, above the YouTube Music view while it is open.
  $effect(() => {
    if (!open) return;
    untrack(() => openOverlay('lyrics'));
    return () => untrack(() => closeOverlay('lyrics'));
  });

  // Opening it moves focus onto the close button; closing it while focus is inside hands
  // focus back to the transport's toggle before the panel turns inert. A panel that is open
  // at start does not take focus.
  let panelEl = $state<HTMLElement>();
  let wasOpen = lyricsPanel.open;

  $effect.pre(() => {
    const now = open;
    untrack(() => {
      if (now === wasOpen) return;
      wasOpen = now;
      if (now) {
        void tick().then(() => {
          if (lyricsPanel.open) panelEl?.querySelector<HTMLElement>('.close')?.focus();
        });
      } else if (panelEl?.contains(document.activeElement)) {
        document.getElementById(LYRICS_TOGGLE_ID)?.focus();
      }
    });
  });

  /** Escape closes the panel from anywhere inside it. */
  function onPanelKeydown(event: KeyboardEvent) {
    if (event.key !== 'Escape' || event.defaultPrevented) return;
    event.preventDefault();
    toggleLyricsPanel(false);
  }
</script>

<!-- Escape bubbles up from the controls inside; the aside itself takes no input. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<aside
  class="panel"
  class:open
  aria-label={t('lyrics.title')}
  inert={!open}
  bind:this={panelEl}
  aria-keyshortcuts="Escape"
  onkeydown={onPanelKeydown}
  style="--c1:{ring.colors[0]};--c2:{ring.colors[1]};--c3:{ring.colors[2]};--c4:{ring.colors[3]}"
>
  <header class="head">
    <h2>{t('lyrics.title')}</h2>
    <button type="button" class="close" aria-label={t('lyrics.close')} title={t('lyrics.close')} onclick={() => toggleLyricsPanel(false)}>
      <Icon name="close" size={16} />
    </button>
  </header>
  {#if open}<LyricsView compact />{/if}
</aside>

<style>
  /* The queue panel's sheet: same place, same look. */
  .panel {
    position: absolute;
    top: calc(var(--header-h) - 4px);
    right: 12px;
    bottom: 14px;
    z-index: 4;
    width: 372px;
    max-width: calc(100% - 24px);
    display: flex;
    flex-direction: column;
    box-sizing: border-box;
    padding: 18px 14px 12px 14px;
    border-radius: var(--radius-l);
    background:
      radial-gradient(120% 40% at 100% 0%, color-mix(in srgb, var(--c4) 7%, transparent), transparent 70%),
      rgba(25, 25, 31, 0.94);
    backdrop-filter: blur(22px) saturate(1.15);
    box-shadow:
      inset 0 0 0 1px rgba(255, 255, 255, 0.08),
      inset 0 1px 0 rgba(255, 255, 255, 0.05),
      -24px 28px 60px -20px rgba(0, 0, 0, 0.85);
    transform: translateX(calc(100% + 24px));
    visibility: hidden;
    transition:
      transform 220ms cubic-bezier(0.22, 0.8, 0.24, 1),
      visibility 0s linear 220ms;
  }

  .panel.open {
    transform: none;
    visibility: visible;
    transition:
      transform 220ms cubic-bezier(0.22, 0.8, 0.24, 1),
      visibility 0s;
  }

  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 0 4px 12px 4px;
  }

  h2 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 19px;
    font-weight: 600;
    letter-spacing: -0.015em;
    line-height: 1.2;
  }

  .close {
    flex: none;
    width: 30px;
    height: 30px;
    margin: -3px -2px 0 0;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--text-dim);
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      color var(--dur) ease-out;
  }

  .close:hover {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
  }

  .close:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 1px;
  }
</style>
