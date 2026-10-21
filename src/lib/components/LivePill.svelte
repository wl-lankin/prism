<script lang="ts">
  import { t } from '../i18n';

  /** "LIVE" in radio blue, with a dot that pulses softly. It takes the place of a seek bar
   *  or of the duration in a list row. */
  let { size = 'normal', title }: { size?: 'small' | 'normal'; title?: string } = $props();
</script>

<span class="pill" class:small={size === 'small'} {title}>
  <span class="dot" aria-hidden="true"></span>
  <span class="text">{t('radio.live')}</span>
</span>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 22px;
    padding: 0 9px 0 8px;
    border-radius: 11px;
    background: color-mix(in srgb, var(--src-radio) 14%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--src-radio) 40%, transparent);
    color: var(--src-radio);
    font-size: 10.5px;
    font-weight: 700;
    letter-spacing: 0.1em;
    line-height: 1;
    user-select: none;
    transition:
      background-color var(--dur) ease-out,
      box-shadow var(--dur) ease-out;
  }

  .pill.small {
    height: 18px;
    padding: 0 7px 0 6px;
    gap: 5px;
    font-size: 9.5px;
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
    box-shadow: 0 0 6px color-mix(in srgb, currentColor 70%, transparent);
  }

  /* Large motion: the pulse is an endless loop, so it stops under reduced motion. */
  @media (prefers-reduced-motion: no-preference) {
    .dot {
      animation: pulse 2.2s ease-in-out infinite;
    }
  }

  @keyframes pulse {
    50% {
      opacity: 0.35;
      transform: scale(0.8);
    }
  }
</style>
