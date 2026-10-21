<script lang="ts">
  import { t } from '../i18n';
  import { coverUrl } from '../ipc';
  import { hideView, reportRect, signIn, ytm } from '../stores/ytm.svelte';

  /** YouTube Music lives in a native child view that Rust lays over the `.stage` below. This
   *  component only keeps the stage's rect reported and shows a calm still while an overlay
   *  hides the child. */

  const HINT_KEY = 'prism.ytmSignInHintDismissed';

  function hintWasDismissed(): boolean {
    try {
      return localStorage.getItem(HINT_KEY) === '1';
    } catch {
      return false;
    }
  }

  let hint = $state(!hintWasDismissed());
  let stage: HTMLElement | undefined = $state();

  function dismissHint() {
    hint = false;
    try {
      localStorage.setItem(HINT_KEY, '1');
    } catch {
      // The hint just comes back next time.
    }
  }

  function report() {
    if (!stage) return;
    const box = stage.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) return;
    reportRect({ x: box.left, y: box.top, width: box.width, height: box.height });
  }

  $effect(() => {
    if (!stage) return;
    report();
    const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(report) : null;
    observer?.observe(stage);
    return () => observer?.disconnect();
  });

  // Leaving the view hides the child; playback goes on.
  $effect(() => () => hideView());

  const loading = $derived(ytm.state.status === 'idle' && !ytm.state.title);
  const still = $derived(ytm.hiddenForOverlay ? coverUrl(ytm.state.coverId, 600) : null);
</script>

<svelte:window onresize={report} />

<section class="ytm" aria-label={t('ytm.title')}>
  {#if hint}
    <div class="hint" role="note">
      <span class="hint-text">{t('ytm.signInHint')}</span>
      <button type="button" class="btn primary" onclick={() => void signIn()}>{t('ytm.signIn')}</button>
      <button type="button" class="btn ghost" onclick={dismissHint}>{t('ytm.hintDismiss')}</button>
    </div>
  {/if}

  <div class="stage" class:loading bind:this={stage}>
    {#if ytm.hiddenForOverlay}
      <div class="still" role="img" aria-label={t('ytm.unavailable')}>
        {#if still}<img src={still} alt="" />{/if}
        <i class="veil"></i>
      </div>
    {/if}
    {#if loading}<i class="hairline" aria-hidden="true"></i>{/if}
  </div>
</section>

<style>
  .ytm {
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 0 16px 16px;
    box-sizing: border-box;
  }

  .hint {
    flex: none;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 8px 8px 16px;
    border-radius: 12px;
    background: color-mix(in srgb, var(--src-youtube) 9%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--src-youtube) 22%, transparent);
    font-size: 13px;
  }

  .hint .btn {
    height: 32px;
    padding: 0 14px;
    font-size: 12.5px;
  }

  .hint-text {
    flex: 1;
    min-width: 0;
  }

  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    border-radius: 14px;
    background: linear-gradient(160deg, color-mix(in srgb, var(--src-youtube) 10%, #14151a), #14151a 70%);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, white 6%, transparent);
  }

  .still {
    position: absolute;
    inset: 0;
    background: linear-gradient(160deg, color-mix(in srgb, var(--src-youtube) 22%, #1a1b21), #15161b 75%);
  }

  .still img {
    position: absolute;
    inset: -8%;
    width: 116%;
    height: 116%;
    object-fit: cover;
    filter: blur(34px) saturate(1.1);
  }

  .veil {
    position: absolute;
    inset: 0;
    background: color-mix(in srgb, #0d0e12 62%, transparent);
  }

  .hairline {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    height: 1px;
    background: linear-gradient(90deg, transparent, var(--src-youtube), transparent);
    background-size: 40% 100%;
    background-repeat: no-repeat;
    opacity: 0.8;
    animation: sweep 1.8s ease-in-out infinite;
  }

  @keyframes sweep {
    from {
      background-position: -40% 0;
    }
    to {
      background-position: 140% 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .hairline {
      animation: none;
      background-size: 100% 100%;
      opacity: 0.5;
    }
  }
</style>
