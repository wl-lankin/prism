<script lang="ts">
  /** A cover, tiny and blurred far past recognition, tinting a view. The square source fills a
   *  small box with the backdrop's own aspect (`background-size: cover`), which is scaled up past
   *  the backdrop's width and height: it covers the area at any size, and far fewer pixels are
   *  blurred than in a full-size image. A new source fades in. */
  let {
    src,
    height,
    spread = 1.3,
  }: {
    src: string;
    /** A header band of this height at the top; without it the backdrop fills its parent. */
    height?: number;
    spread?: number;
  } = $props();

  /** The small box's width before scaling. */
  const PX = 64;
  let width = $state(1000);
  let measured = $state(520);
  const boxHeight = $derived((PX * measured) / Math.max(1, width));
</script>

<div
  class="backdrop"
  class:full={height === undefined}
  aria-hidden="true"
  style:height={height === undefined ? null : `calc(${height}px + var(--view-top, 0px))`}
  bind:clientWidth={width}
  bind:clientHeight={measured}
>
  {#key src}
    <div
      class="image"
      style="background-image:url('{src}'); height:{boxHeight}px; margin-top:{-boxHeight / 2}px; --k:{(width * spread) / PX}"
    ></div>
  {/key}
</div>

<style>
  .backdrop {
    position: absolute;
    inset: 0 0 auto 0;
    overflow: hidden;
    /* WebKit lets the scaled, blurred image escape `overflow: hidden` for the first frames
       of its fade-in, painting over the sidebar (owner, 2026-10-22); paint containment
       clips it from the first frame on. */
    contain: paint;
    pointer-events: none;
    /* Runs from the card's top edge (behind the floating title bar, `--view-top`) and fades
       out into the list below, so it has no hard edge. Without a floating bar above it, it
       fades in under the title bar instead. */
    mask-image: linear-gradient(
      to bottom,
      rgba(0, 0, 0, var(--top-alpha, 0.25)) 0,
      #000 calc(110px + var(--view-top, 0px)),
      rgba(0, 0, 0, 0.55) 55%,
      transparent 100%
    );
  }

  /* Behind a whole view: it never fades out completely. */
  .backdrop.full {
    inset: 0;
    mask-image: linear-gradient(
      to bottom,
      rgba(0, 0, 0, var(--top-alpha, 0.3)) 0,
      #000 calc(90px + var(--view-top, 0px)),
      rgba(0, 0, 0, 0.6) 62%,
      rgba(0, 0, 0, 0.18) 100%
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

  .full .image {
    filter: blur(4px) saturate(1.4);
    opacity: 0.5;
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }
</style>
