<script lang="ts">
  import { coverUrl, type CoverSize } from '../ipc';
  import Icon from './Icon.svelte';

  let {
    coverId,
    size = 48,
    radius = 'var(--radius-s)',
    alt = '',
    fluid = false,
    cache,
  }: {
    coverId: string | null;
    /** Rendered size in CSS px; the cached image size is picked from it. */
    size?: number;
    radius?: string;
    alt?: string;
    /** Fills its container's width (a square) instead of `size`. */
    fluid?: boolean;
    /** Forces a cached size instead of picking one from `size`. */
    cache?: CoverSize;
  } = $props();

  // Covers are cached at 96, 300 and 600 px; pick the smallest that stays sharp on 2x screens.
  const px: CoverSize = $derived(cache ?? (size * 2 <= 96 ? 96 : size * 2 <= 300 ? 300 : 600));
  const src = $derived(coverUrl(coverId, px));

  let loaded = $state(false);
  let failed = $state(false);

  $effect.pre(() => {
    // A new image starts hidden until it has loaded.
    void src;
    loaded = false;
    failed = false;
  });
</script>

<div class="cover" class:fluid style="--s:{size}px; --r:{radius}">
  <div class="empty" aria-hidden="true">
    <Icon name="songs" size={Math.round(size * 0.38)} />
  </div>
  {#if src && !failed}
    <img
      {src}
      {alt}
      class:loaded
      draggable="false"
      decoding="async"
      onload={() => (loaded = true)}
      onerror={() => (failed = true)}
    />
  {/if}
</div>

<style>
  .cover {
    position: relative;
    width: var(--s);
    height: var(--s);
    flex: none;
    overflow: hidden;
    border-radius: var(--r);
    background: linear-gradient(145deg, #25252c, #18181d);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  .cover.fluid {
    width: 100%;
    height: auto;
    aspect-ratio: 1;
  }

  .empty {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: rgba(243, 243, 245, 0.28);
  }

  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    opacity: 0;
    transition: opacity var(--dur) ease-out;
  }

  img.loaded {
    opacity: 1;
  }
</style>
