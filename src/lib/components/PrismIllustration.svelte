<script lang="ts">
  /** The Prism mark exactly as in the app icon (assets/logo.svg), without the icon's tile: the
   *  same beam, prism and three spectrum rays (owner, 2026-10-21). */
  let { width = 220 }: { width?: number } = $props();
</script>

<div class="art" style="--w:{width}px" aria-hidden="true">
  <svg viewBox="80 210 880 590" width={width} height={(width * 590) / 880}>
    <defs>
      <linearGradient id="prism-spectrum" gradientUnits="userSpaceOnUse" x1="590" y1="400" x2="900" y2="720">
        <stop offset="0" stop-color="#ff4d6d" />
        <stop offset="0.36" stop-color="#ffc94d" />
        <stop offset="0.68" stop-color="#4dffb2" />
        <stop offset="1" stop-color="#4da6ff" />
      </linearGradient>
      <linearGradient id="prism-glass" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#ffffff" stop-opacity="0.09" />
        <stop offset="1" stop-color="#ffffff" stop-opacity="0.02" />
      </linearGradient>
    </defs>
    <g fill="none" stroke-linecap="round" stroke-linejoin="round" stroke-width="58">
      <path d="M566 456 Q 760 446 900 354" stroke="url(#prism-spectrum)" />
      <path d="M614 540 Q 770 540 918 540" stroke="url(#prism-spectrum)" />
      <path d="M662 624 Q 790 634 900 726" stroke="url(#prism-spectrum)" />
      <path d="M116 540 L286 540" stroke="#f3f3f5" />
      <!-- The icon's tile color under the glass, so the prism looks the same on any page. -->
      <path d="M450 256 L739 756 L161 756 Z" fill="#141418" stroke="none" />
      <path d="M450 256 L739 756 L161 756 Z" fill="url(#prism-glass)" stroke="#f3f3f5" />
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

  svg {
    position: relative;
    display: block;
    overflow: visible;
  }
</style>
