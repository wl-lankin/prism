<script lang="ts">
  import { t } from '../i18n';

  /** Three spectral bars marking the playing track; they hold still while paused. */
  let { playing = true, size = 14 }: { playing?: boolean; size?: number } = $props();
</script>

<span class="bars" class:playing style="--s:{size}px" role="img" aria-label={t('album.nowPlaying')}>
  <i></i><i></i><i></i>
</span>

<style>
  .bars {
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    width: var(--s);
    height: var(--s);
    flex: none;
  }

  i {
    flex: 1;
    height: 100%;
    border-radius: 1px;
    transform-origin: bottom;
    transform: scaleY(0.4);
  }

  i:nth-child(1) {
    background: var(--spec-1);
    transform: scaleY(0.55);
  }
  i:nth-child(2) {
    background: var(--spec-3);
    transform: scaleY(0.9);
  }
  i:nth-child(3) {
    background: var(--spec-4);
    transform: scaleY(0.4);
  }

  .playing i {
    animation: bounce 0.9s ease-in-out infinite alternate;
  }
  .playing i:nth-child(2) {
    animation-duration: 0.7s;
    animation-delay: -0.3s;
  }
  .playing i:nth-child(3) {
    animation-duration: 1.1s;
    animation-delay: -0.6s;
  }

  @keyframes bounce {
    from {
      transform: scaleY(0.25);
    }
    to {
      transform: scaleY(1);
    }
  }
</style>
