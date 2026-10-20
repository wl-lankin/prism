<script lang="ts">
  /** The Prism mark drawn as an illustration (no app-icon tile): white light enters the glass
   *  prism and leaves as the four spectrum colors, over a soft glow of the same colors. */
  let { width = 220 }: { width?: number } = $props();
</script>

<div class="art" style="--w:{width}px" aria-hidden="true">
  <div class="glow"></div>
  <svg viewBox="80 210 880 600" width={width} height={(width * 600) / 880}>
    <defs>
      <linearGradient id="prism-glass" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#ffffff" stop-opacity="0.2" />
        <stop offset="1" stop-color="#ffffff" stop-opacity="0.05" />
      </linearGradient>
      <linearGradient id="prism-beam" x1="0" y1="0" x2="1" y2="0">
        <stop offset="0" stop-color="#f3f3f5" stop-opacity="0" />
        <stop offset="0.45" stop-color="#f3f3f5" />
      </linearGradient>
    </defs>
    <g fill="none" stroke-linecap="round" stroke-linejoin="round">
      <!-- Four rays fan out of the prism's right face, one per spectrum color. -->
      <path d="M560 470 Q 740 450 910 330" stroke="#ff4d6d" stroke-width="30" />
      <path d="M596 530 Q 760 520 930 470" stroke="#ffc94d" stroke-width="30" />
      <path d="M632 592 Q 780 600 930 620" stroke="#4dffb2" stroke-width="30" />
      <path d="M668 654 Q 790 680 900 760" stroke="#4da6ff" stroke-width="30" />
      <!-- White light in from the left. -->
      <path d="M96 556 L 300 540" stroke="url(#prism-beam)" stroke-width="30" />
      <!-- The glass: a solid body (so the glow does not show through unevenly) with a sheen,
           both filling the triangle right up to its white edge. -->
      <path d="M450 256 L739 756 L161 756 Z" fill="#1d1d24" />
      <path d="M450 256 L739 756 L161 756 Z" fill="url(#prism-glass)" stroke="#f3f3f5" stroke-width="34" />
    </g>
  </svg>
</div>

<style>
  .art {
    position: relative;
    width: var(--w);
    display: grid;
    place-items: center;
  }

  /* The spectrum, blurred into a soft pool behind the prism and its rays. */
  .glow {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 78%;
    aspect-ratio: 1.4;
    transform: translate(-42%, -50%);
    border-radius: 50%;
    background: conic-gradient(
      from 200deg,
      var(--spec-1),
      var(--spec-2),
      var(--spec-3),
      var(--spec-4),
      var(--spec-1)
    );
    filter: blur(40px);
    opacity: 0.42;
  }

  svg {
    position: relative;
    display: block;
    overflow: visible;
  }
</style>
