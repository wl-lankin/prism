<script lang="ts">
  import { coverUrl } from '../ipc';
  import type { Station } from '../types';

  /** A station's logo contained on a raised tile; its initials in the radio blue stand in
   *  until a logo loads and when none does. */
  let {
    station,
    size = 40,
    radius = '8px',
  }: {
    station: Pick<Station, 'name' | 'coverId'>;
    size?: number;
    radius?: string;
  } = $props();

  const src = $derived(coverUrl(station.coverId, size * 2 <= 96 ? 96 : 300));
  const initials = $derived(
    station.name
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((word) => [...word][0])
      .join('')
      .toUpperCase(),
  );

  let loadedSrc: string | null = $state(null);
  let failedSrc: string | null = $state(null);
</script>

<div class="art" style="--s:{size}px; --r:{radius}">
  <span class="initials" class:hidden={loadedSrc === src} aria-hidden="true">{initials}</span>
  {#key src}
    {#if src && failedSrc !== src}
      <img
        {src}
        alt=""
        class:loaded={loadedSrc === src}
        draggable="false"
        decoding="async"
        onload={() => (loadedSrc = src)}
        onerror={() => (failedSrc = src)}
      />
    {/if}
  {/key}
</div>

<style>
  .art {
    position: relative;
    width: var(--s);
    height: var(--s);
    flex: none;
    overflow: hidden;
    border-radius: var(--r);
    background: var(--raised);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  .initials {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    font-size: calc(var(--s) * 0.34);
    font-weight: 650;
    letter-spacing: 0.02em;
    color: var(--src-radio);
    transition: opacity var(--dur) ease-out;
  }

  .initials.hidden {
    opacity: 0;
  }

  img {
    position: absolute;
    inset: 12%;
    width: 76%;
    height: 76%;
    object-fit: contain;
    opacity: 0;
    transition: opacity var(--dur) ease-out;
  }

  img.loaded {
    opacity: 1;
  }
</style>
