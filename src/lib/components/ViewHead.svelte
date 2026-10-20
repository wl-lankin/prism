<script lang="ts">
  import type { Snippet } from 'svelte';

  /** The heading of a library view: a title, a count beside it, optional actions on the right. */
  let { title, meta, actions }: { title: string; meta?: string; actions?: Snippet } = $props();
</script>

<header class="viewhead">
  <h1>{title}</h1>
  {#if meta}<span class="meta">{meta}</span>{/if}
  {#if actions}<div class="actions">{@render actions()}</div>{/if}
</header>

<style>
  .viewhead {
    display: flex;
    align-items: baseline;
    gap: 14px;
    padding: 6px 0 22px;
  }

  h1 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 32px;
    font-weight: 600;
    letter-spacing: -0.02em;
    line-height: 1.1;
  }

  .meta {
    font-size: 13px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .actions {
    margin-left: auto;
    align-self: center;
    display: flex;
    gap: 10px;
  }
</style>
