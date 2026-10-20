<script lang="ts">
  /** A cover, tiny and blurred far past recognition, tinting the top of a view. The square
   *  source fills a small box with the header's own aspect (`background-size: cover`), which is
   *  scaled up past the header's width and height: it covers the header at any width, and far
   *  fewer pixels are blurred than in a full-size image. */
  let { src, height = 520, spread = 1.3 }: { src: string; height?: number; spread?: number } = $props();

  /** The small box's width before scaling. */
  const PX = 64;
  let width = $state(1000);
  const boxHeight = $derived((PX * height) / Math.max(1, width));
</script>

<div class="backdrop" aria-hidden="true" style="height:{height}px" bind:clientWidth={width}>
  <div
    class="image"
    style="background-image:url('{src}'); height:{boxHeight}px; margin-top:{-boxHeight / 2}px; --k:{(width * spread) / PX}"
  ></div>
</div>

<style>
  .backdrop {
    position: absolute;
    inset: 0 0 auto 0;
    overflow: hidden;
    pointer-events: none;
    /* Fades in under the title bar and out into the list below, so it has no hard edge. */
    mask-image: linear-gradient(
      to bottom,
      rgba(0, 0, 0, 0.25) 0,
      #000 110px,
      rgba(0, 0, 0, 0.55) 55%,
      transparent 100%
    );
  }

  .image {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 64px;
    margin-left: -32px;
    background-size: cover;
    background-position: center;
    background-repeat: no-repeat;
    transform: scale(var(--k, 20));
    filter: blur(4px) saturate(1.35);
    opacity: 0.55;
    animation: fade var(--dur-l) ease-out;
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }
</style>
