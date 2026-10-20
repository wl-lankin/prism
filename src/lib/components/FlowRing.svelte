<script lang="ts" module>
  import { SPECTRUM, type RingColors } from '../palette';

  export type RingMode = 'flow' | 'play' | 'pause';
  export type { RingColors };

  function rgb(hex: string): [number, number, number] {
    const n = parseInt(hex.replace('#', '').slice(0, 6), 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }

  /** The gradient's color at `p` (0..1), with the four stops at 0, 1/3, 2/3 and 1. */
  export function colorAt(colors: RingColors, p: number): string {
    const x = Math.min(Math.max(p, 0), 1) * 3;
    const i = Math.min(Math.floor(x), 2);
    const f = x - i;
    const [a, b] = [rgb(colors[i]), rgb(colors[i + 1])];
    const c = a.map((v, k) => Math.round(v + (b[k] - v) * f));
    return `rgb(${c[0]}, ${c[1]}, ${c[2]})`;
  }
</script>

<script lang="ts">
  import { t } from '../i18n';

  let {
    mode,
    progress = 0,
    colors = SPECTRUM,
    size = 84,
    level,
    onactivate,
    oncontextmenu,
  }: {
    /** 'flow' while idle; 'play' shows the play glyph (paused), 'pause' the pause glyph (playing). */
    mode: RingMode;
    /** 0..1, drawn as the spectral arc in play and pause mode. */
    progress?: number;
    /** A color change fades over 600 ms. */
    colors?: RingColors;
    size?: number;
    /** 0..1, the loudness of what plays. While playing it drives the glow (in place of the
     *  slow breathing); left out, the ring breathes on its own. */
    level?: number;
    onactivate?: () => void;
    oncontextmenu?: (event: MouseEvent) => void;
  } = $props();

  const p = $derived(mode === 'flow' ? 1 : Math.min(Math.max(progress || 0, 0), 1));
  const thickness = $derived(Math.max(3, Math.round(size / 17)));
  const metered = $derived(level !== undefined && mode === 'pause');
  const loud = $derived(metered ? Math.min(Math.max(level || 0, 0), 1) : 0);
  const label = $derived(mode === 'flow' ? t('ring.flowHint') : mode === 'pause' ? t('ring.pause') : t('ring.play'));
  const style = $derived(
    [
      `--size:${size}px`,
      `--t:${thickness}px`,
      `--deg:${(p * 360).toFixed(2)}deg`,
      `--c1:${colors[0]}`,
      `--c2:${colors[1]}`,
      `--c3:${colors[2]}`,
      `--c4:${colors[3]}`,
      `--head:${colorAt(colors, p)}`,
      ...(metered ? [`--glow-o:${(0.35 + 0.4 * loud).toFixed(3)}`, `--glow-s:${(1 + 0.12 * loud).toFixed(3)}`] : []),
    ].join(';'),
  );
</script>

<button
  type="button"
  class="ring"
  class:flow={mode === 'flow'}
  class:playing={mode === 'pause'}
  class:metered
  {style}
  aria-label={label}
  title={label}
  onclick={() => onactivate?.()}
  oncontextmenu={(event) => {
    event.preventDefault();
    oncontextmenu?.(event);
  }}
>
  <span class="glow" aria-hidden="true"></span>
  <span class="track" aria-hidden="true"></span>
  <span class="arc" aria-hidden="true"></span>
  {#if mode !== 'flow' && p > 0}
    <span class="cap start" aria-hidden="true"></span>
    {#if p < 0.995}
      <!-- At the very end the head would sit on the start cap. -->
      <span class="cap head" aria-hidden="true"></span>
    {/if}
  {/if}
  <span class="disc">
    {#if mode === 'pause'}
      <svg class="glyph" viewBox="0 0 24 24" aria-hidden="true">
        <rect x="6.5" y="5" width="3.6" height="14" rx="1.3" />
        <rect x="13.9" y="5" width="3.6" height="14" rx="1.3" />
      </svg>
    {:else}
      <svg class="glyph play" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M8 5.6v12.8a1 1 0 0 0 1.5.86l10.4-6.4a1 1 0 0 0 0-1.72L9.5 4.74A1 1 0 0 0 8 5.6z" />
      </svg>
    {/if}
    {#if mode === 'flow'}
      <span class="label">{t('ring.flow')}</span>
    {/if}
  </span>
</button>

<style>
  @property --spin {
    syntax: '<angle>';
    inherits: true;
    initial-value: 0deg;
  }

  /* Registered so they interpolate: a new cover's colors fade in instead of jumping. */
  @property --c1 {
    syntax: '<color>';
    inherits: true;
    initial-value: #ff4d6d;
  }

  @property --c2 {
    syntax: '<color>';
    inherits: true;
    initial-value: #ffc94d;
  }

  @property --c3 {
    syntax: '<color>';
    inherits: true;
    initial-value: #4dffb2;
  }

  @property --c4 {
    syntax: '<color>';
    inherits: true;
    initial-value: #4da6ff;
  }

  @property --head {
    syntax: '<color>';
    inherits: true;
    initial-value: #ff4d6d;
  }

  .ring {
    --spin: 0deg;
    --fade: 600ms;
    position: relative;
    width: var(--size);
    height: var(--size);
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--text);
    cursor: pointer;
    -webkit-tap-highlight-color: transparent;
    /* A color fade is not large motion: it keeps running under reduced motion. */
    transition:
      transform var(--dur) ease-out,
      --c1 var(--fade) ease,
      --c2 var(--fade) ease,
      --c3 var(--fade) ease,
      --c4 var(--fade) ease,
      --head var(--fade) ease;
  }

  .ring:active {
    transform: scale(0.965);
  }

  .ring:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 4px;
  }

  .glow,
  .track,
  .arc {
    position: absolute;
    inset: 0;
    border-radius: 50%;
  }

  /* The spectrum, blurred, under everything: the ring's light on the window. Its reach
     (blur, inset, offset) sets the `glow` values in window-shape.json: the transparent
     window's click-through shape keeps that much room around the ring, so measure the glow
     again and update the JSON when changing it. */
  .glow {
    inset: 10%;
    background: conic-gradient(from var(--spin), var(--c1), var(--c2), var(--c3), var(--c4), var(--c1));
    filter: blur(calc(var(--size) * 0.2));
    opacity: var(--glow-o, 0.5);
    transform: translateY(12%) scale(var(--glow-s, 1));
    transition:
      opacity var(--dur) ease-out,
      transform var(--dur) ease-out;
  }

  .ring:hover .glow {
    opacity: calc(var(--glow-o, 0.5) + 0.2);
  }

  .track {
    background: rgba(255, 255, 255, 0.12);
    mask: radial-gradient(farthest-side, transparent calc(100% - var(--t)), #000 calc(100% - var(--t) + 0.6px));
  }

  .arc {
    background: conic-gradient(var(--c1), var(--c2) 120deg, var(--c3) 240deg, var(--c4) 360deg);
    mask:
      radial-gradient(farthest-side, transparent calc(100% - var(--t)), #000 calc(100% - var(--t) + 0.6px)),
      conic-gradient(#000 var(--deg), transparent var(--deg));
    mask-composite: intersect;
  }

  .flow .arc {
    background: conic-gradient(from var(--spin), var(--c1), var(--c2), var(--c3), var(--c4), var(--c1));
    mask: radial-gradient(farthest-side, transparent calc(100% - var(--t)), #000 calc(100% - var(--t) + 0.6px));
  }

  /* Round ends for the progress arc. */
  .cap {
    position: absolute;
    left: calc(50% - var(--t) / 2);
    top: 0;
    width: var(--t);
    height: var(--t);
    border-radius: 50%;
    transform-origin: 50% calc(var(--size) / 2);
  }

  .cap.start {
    background: var(--c1);
  }

  .cap.head {
    background: var(--head);
    transform: rotate(var(--deg));
    box-shadow: 0 0 calc(var(--t) * 2) var(--head);
  }

  .disc {
    position: absolute;
    inset: calc(var(--t) + 2px);
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: calc(var(--size) * 0.02);
    border-radius: 50%;
    background: radial-gradient(120% 90% at 50% 18%, #24242b 0%, #17171c 55%, #121216 100%);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.07),
      inset 0 -8px 16px rgba(0, 0, 0, 0.35);
    transition: background-color var(--dur) ease-out, filter var(--dur) ease-out;
  }

  .ring:hover .disc {
    filter: brightness(1.22);
  }

  .glyph {
    width: calc(var(--size) * 0.27);
    height: calc(var(--size) * 0.27);
    fill: var(--text);
  }

  .glyph.play {
    margin-left: calc(var(--size) * 0.025);
  }

  .flow .glyph {
    width: calc(var(--size) * 0.22);
    height: calc(var(--size) * 0.22);
    margin-top: calc(var(--size) * 0.04);
  }

  .label {
    font-family: var(--font-display);
    font-size: calc(var(--size) * 0.12);
    font-weight: 600;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    margin-left: 0.16em;
    color: var(--text);
    opacity: 0.92;
  }

  /* Large motion: the idle spectrum turns slowly, and without a level the glow breathes on
     its own while playing. Both stop under reduced motion. The hover transitions, the color
     fades and the level-driven glow (small, 120 ms steps that follow the music) keep running. */
  @media (prefers-reduced-motion: no-preference) {
    .flow {
      animation: spin 14s linear infinite;
    }

    .playing:not(.metered) .glow {
      animation: breathe 3.6s ease-in-out infinite;
    }
  }

  @keyframes spin {
    to {
      --spin: 360deg;
    }
  }

  @keyframes breathe {
    50% {
      opacity: 0.6;
    }
  }
</style>
