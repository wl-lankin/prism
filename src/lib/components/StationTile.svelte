<script lang="ts" module>
  /** Up to two letters for a station without a logo: the first letters of its first two words,
   *  or the first two characters of a single word. */
  export function initials(name: string): string {
    const [first, second] = name.match(/[\p{L}\p{N}]+/gu) ?? [];
    if (!first) return '?';
    const letters = second ? `${[...first][0]}${[...second][0]}` : [...first].slice(0, 2).join('');
    return letters.toUpperCase();
  }
</script>

<script lang="ts">
  import { coverUrl } from '../ipc';

  /** A station's logo on a raised square, contained so nothing is cropped. Without a logo, or
   *  when it does not load, the station's initials in radio blue. */
  let { name, coverId, size = 40 }: { name: string; coverId: string | null; size?: number } = $props();

  const src = $derived(coverUrl(coverId, 96));
  let failedSrc: string | null = $state(null);
  let loadedSrc: string | null = $state(null);
  const showImage = $derived(!!src && failedSrc !== src);
  const logo = $derived(showImage && loadedSrc === src);
</script>

<div class="tile" class:logo style="--s:{size}px">
  {#if !logo}
    <span class="initials" aria-hidden="true">{initials(name)}</span>
  {/if}
  <!-- A fresh element per image, so the old logo never shows under a new id. -->
  {#key src}
    {#if showImage}
      <img
        {src}
        alt=""
        class:loaded={logo}
        draggable="false"
        decoding="async"
        onload={() => (loadedSrc = src)}
        onerror={() => (failedSrc = src)}
      />
    {/if}
  {/key}
</div>

<style>
  .tile {
    position: relative;
    width: var(--s);
    height: var(--s);
    flex: none;
    display: grid;
    place-items: center;
    overflow: hidden;
    border-radius: 8px;
    background: var(--surface);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
    transition: background-color var(--dur) ease-out;
  }

  .tile.logo {
    background: var(--raised);
  }

  .initials {
    font-family: var(--font-display);
    font-size: calc(var(--s) * 0.34);
    font-weight: 600;
    letter-spacing: 0.02em;
    color: var(--src-radio);
  }

  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    padding: 4px;
    box-sizing: border-box;
    object-fit: contain;
    opacity: 0;
    transition: opacity var(--dur) ease-out;
  }

  img.loaded {
    opacity: 1;
  }
</style>
