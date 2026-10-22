<script lang="ts">
  import type { CoverSize } from '../ipc';
  import Cover from './Cover.svelte';
  import Icon from './Icon.svelte';

  /** A playlist's art: four covers in a 2 x 2 mosaic, the first cover alone when there are
   *  fewer, and an empty playlist a quiet tile lit by one spectrum color (picked by its id). */
  let {
    coverIds,
    seed = 0,
    size = 48,
    fluid = false,
    radius = '10px',
    cache = 300,
  }: {
    coverIds: string[];
    /** Picks the empty tile's color, so empty playlists side by side differ. */
    seed?: number;
    size?: number;
    fluid?: boolean;
    radius?: string;
    /** The cached size for a single cover; mosaic tiles use the next size down. */
    cache?: CoverSize;
  } = $props();

  const mosaic = $derived(coverIds.length >= 4);
  const tileCache: CoverSize = $derived(cache === 600 ? 300 : 96);
  const tint = $derived(`var(--spec-${(Math.abs(seed) % 4) + 1})`);
</script>

<div class="art" class:fluid style="--s:{size}px; --r:{radius}; --tint:{tint}" aria-hidden="true">
  {#if mosaic}
    <div class="grid">
      {#each coverIds.slice(0, 4) as coverId (coverId)}
        <Cover {coverId} fluid radius="0" cache={tileCache} />
      {/each}
    </div>
  {:else if coverIds.length}
    <Cover coverId={coverIds[0]} fluid radius="0" {cache} />
  {:else}
    <div class="empty">
      <Icon name="playlists" size={fluid ? 34 : Math.max(14, Math.round(size * 0.36))} />
    </div>
  {/if}
</div>

<style>
  .art {
    position: relative;
    width: var(--s);
    height: var(--s);
    flex: none;
    overflow: hidden;
    border-radius: var(--r);
    background: #1a1a1f;
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  .art.fluid {
    width: 100%;
    height: auto;
    aspect-ratio: 1;
  }

  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
  }

  /* A hairline between the tiles keeps four covers reading as one object. */
  .grid :global(.cover) {
    box-shadow: none;
  }

  .grid::after {
    content: '';
    position: absolute;
    inset: 0;
    pointer-events: none;
    background:
      linear-gradient(rgba(13, 13, 16, 0.55), rgba(13, 13, 16, 0.55)) center / 1px 100% no-repeat,
      linear-gradient(rgba(13, 13, 16, 0.55), rgba(13, 13, 16, 0.55)) center / 100% 1px no-repeat;
  }

  .empty {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: color-mix(in srgb, var(--tint) 70%, var(--text));
    background:
      radial-gradient(120% 90% at 0% 0%, color-mix(in srgb, var(--tint) 26%, transparent), transparent 62%),
      radial-gradient(90% 80% at 100% 100%, color-mix(in srgb, var(--tint) 10%, transparent), transparent 70%),
      linear-gradient(145deg, #25252c, #17171c);
  }
</style>
