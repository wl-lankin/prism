<script lang="ts">
  import { t } from '../i18n';
  import type { Source } from '../types';

  /** Where an item comes from: a small dot in the source color, ringed in the surface color
   *  so it reads on any cover. The parent places it (`.source-badge`); on its own it sits
   *  inline. The source name is its accessible name and its tooltip. */
  let { source, size = 10 }: { source: Source; size?: number } = $props();

  const name = $derived(t(`source.${source}`));
</script>

<span
  class="source-badge"
  role="img"
  aria-label={name}
  title={name}
  data-source={source}
  style="--badge:{size}px;--badge-color:var(--src-{source})"
></span>

<style>
  .source-badge {
    display: inline-block;
    flex: none;
    box-sizing: border-box;
    width: var(--badge);
    height: var(--badge);
    border-radius: 50%;
    background: var(--badge-color);
    /* The ring keeps the dot crisp on a busy cover; the glow is barely there. */
    box-shadow:
      0 0 0 1.5px var(--surface),
      0 0 8px color-mix(in srgb, var(--badge-color) 32%, transparent);
    pointer-events: auto;
  }
</style>
