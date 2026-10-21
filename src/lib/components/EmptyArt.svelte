<script lang="ts" module>
  export type EmptyKind = 'songs' | 'albums' | 'artists' | 'stations' | 'playlists' | 'library' | 'search';

  const RED = '#ff4d6d';
  const YELLOW = '#ffc94d';
  const GREEN = '#4dffb2';
  const BLUE = '#4da6ff';

  /** Paths on the 24 px grid of Icon.svelte, so the glyph matches the app's line icons. */
  const GLYPHS: Record<EmptyKind, string> = {
    songs: '<path d="M9 17.5V6l10-2v11.5"/><circle cx="6.5" cy="17.5" r="2.5"/><circle cx="16.5" cy="15.5" r="2.5"/>',
    albums: '<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="2.2"/>',
    artists: '<circle cx="12" cy="8.5" r="3.5"/><path d="M5 20a7 7 0 0 1 14 0"/>',
    stations:
      '<circle cx="12" cy="9.5" r="1.6"/><path d="M8.6 6.1a4.8 4.8 0 0 0 0 6.8M15.4 6.1a4.8 4.8 0 0 1 0 6.8M6 3.5a8.5 8.5 0 0 0 0 12M18 3.5a8.5 8.5 0 0 1 0 12"/><path d="M12 11.1 9.2 20.5M12 11.1l2.8 9.4M10.1 17.5h3.8"/>',
    playlists:
      '<path d="M4 6.5h12M4 11.5h12M4 16.5h7"/><path d="M18 18.5V12l2.5 1"/><circle cx="16.2" cy="18.5" r="1.8"/>',
    library:
      '<path d="M4 7.5A1.5 1.5 0 0 1 5.5 6h4l2 2h7A1.5 1.5 0 0 1 20 9.5v8a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 17.5z"/><path d="M13 15.6v-4.4l2.6.9"/><circle cx="11.6" cy="15.6" r="1.4"/>',
    search: '<circle cx="10.5" cy="10.5" r="6"/><path d="m15 15 5 5"/>',
  };

  interface Look {
    accent: string;
    /** The refracted line along the front slab's top edge, left to right. */
    line: string[];
    /** The faint tints of the back slabs, left and right. */
    tints: [string, string];
    glow: number;
  }

  const LOOKS: Record<EmptyKind, Look> = {
    songs: { accent: RED, line: [RED, RED, YELLOW], tints: [BLUE, YELLOW], glow: 0.16 },
    albums: { accent: YELLOW, line: [RED, YELLOW, GREEN], tints: [RED, GREEN], glow: 0.14 },
    artists: { accent: GREEN, line: [YELLOW, GREEN, BLUE], tints: [YELLOW, BLUE], glow: 0.13 },
    stations: { accent: BLUE, line: [GREEN, BLUE, BLUE], tints: [GREEN, RED], glow: 0.17 },
    playlists: { accent: RED, line: [RED, YELLOW, GREEN, BLUE], tints: [BLUE, GREEN], glow: 0.14 },
    library: { accent: '#cccccc', line: ['#cccccc', RED, YELLOW, GREEN, BLUE], tints: [BLUE, RED], glow: 0.1 },
    search: { accent: '#f3f3f5', line: ['#f3f3f5', '#ffffff', '#f3f3f5'], tints: [BLUE, GREEN], glow: 0.07 },
  };

  let next = 0;
</script>

<script lang="ts">
  /** The empty-state illustration: three frosted glass slabs, the kind's line glyph on the front
   *  one and a short refracted spectrum along its top edge. Purely decorative. */
  let { kind, accent, width = 136 }: { kind: EmptyKind; accent?: string; width?: number } = $props();

  const id = `empty-art-${next++}`;
  const look = $derived(LOOKS[kind]);
  const glow = $derived(accent ?? look.accent);
  const line = $derived(accent ? [look.tints[0], accent, look.tints[1]] : look.line);
  const stops = $derived(
    line.map((color, i) => ({ color, offset: 0.14 + (0.72 * i) / Math.max(1, line.length - 1) })),
  );
</script>

<svg
  class="empty-art"
  data-kind={kind}
  width={width}
  height={(width * 108) / 136}
  viewBox="0 0 136 108"
  fill="none"
  aria-hidden="true"
>
  <defs>
    <radialGradient id="{id}-glow" cx="0.5" cy="0.5" r="0.5">
      <!-- Colors go through style so an accent can be a CSS variable. -->
      <stop offset="0" style="stop-color:{glow};stop-opacity:{look.glow}" />
      <stop offset="0.55" style="stop-color:{glow};stop-opacity:{look.glow * 0.35}" />
      <stop offset="1" style="stop-color:{glow};stop-opacity:0" />
    </radialGradient>
    <radialGradient id="{id}-shadow" cx="0.5" cy="0.5" r="0.5">
      <stop offset="0" stop-color="#000" stop-opacity="0.55" />
      <stop offset="1" stop-color="#000" stop-opacity="0" />
    </radialGradient>
    <linearGradient id="{id}-base" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#26262d" />
      <stop offset="1" stop-color="#18181d" />
    </linearGradient>
    <linearGradient id="{id}-frost" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#fff" stop-opacity="0.09" />
      <stop offset="1" stop-color="#fff" stop-opacity="0.02" />
    </linearGradient>
    {#each look.tints as tint, i (i)}
      <linearGradient id="{id}-tint{i}" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0" stop-color={tint} stop-opacity="0.2" />
        <stop offset="1" stop-color={tint} stop-opacity="0.03" />
      </linearGradient>
    {/each}
    <linearGradient id="{id}-line" gradientUnits="userSpaceOnUse" x1="44" y1="0" x2="92" y2="0">
      <stop offset="0" style="stop-color:{line[0]};stop-opacity:0" />
      {#each stops as stop, i (i)}
        <stop offset={stop.offset} style="stop-color:{stop.color};stop-opacity:0.95" />
      {/each}
      <stop offset="1" style="stop-color:{line[line.length - 1]};stop-opacity:0" />
    </linearGradient>
  </defs>

  <ellipse cx="68" cy="56" rx="64" ry="50" fill="url(#{id}-glow)" />
  <ellipse cx="68" cy="94" rx="38" ry="6" fill="url(#{id}-shadow)" />

  {#each [{ turn: 'rotate(-11 68 58) translate(-11 -2)', tint: 0 }, { turn: 'rotate(9 68 58) translate(11 -3)', tint: 1 }] as back (back.tint)}
    <g transform={back.turn}>
      <rect x="36" y="26" width="64" height="64" rx="14" fill="url(#{id}-base)" />
      <rect x="36" y="26" width="64" height="64" rx="14" fill="url(#{id}-tint{back.tint})" />
      <rect x="36" y="26" width="64" height="64" rx="14" fill="url(#{id}-frost)" />
      <rect x="36.5" y="26.5" width="63" height="63" rx="13.5" stroke="#fff" stroke-opacity="0.07" />
    </g>
  {/each}

  <rect x="36" y="26" width="64" height="64" rx="14" fill="url(#{id}-base)" />
  <rect x="36" y="26" width="64" height="64" rx="14" fill="url(#{id}-frost)" />
  <rect x="36.5" y="26.5" width="63" height="63" rx="13.5" stroke="#fff" stroke-opacity="0.11" />

  <path d="M44 27h48" stroke="url(#{id}-line)" stroke-width="3" stroke-linecap="round" opacity="0.3" />
  <path d="M44 26.75h48" stroke="url(#{id}-line)" stroke-width="1.25" stroke-linecap="round" />

  <g
    class="glyph"
    transform="translate(53 43) scale(1.25)"
    stroke="#f3f3f5"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    {@html GLYPHS[kind]}
  </g>
</svg>

<style>
  .empty-art {
    display: block;
    flex: none;
    overflow: visible;
  }

  .glyph :global(*) {
    vector-effect: non-scaling-stroke;
  }
</style>
