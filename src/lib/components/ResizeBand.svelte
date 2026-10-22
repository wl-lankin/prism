<script lang="ts">
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { windowState } from '../stores/window.svelte';

  /** The bottom edge of the window is the click-through strip, so the system's own resize
   *  border there is gone. This band along the card's bottom edge (5 px, corners 12 px)
   *  resizes the window instead. The centre keeps `gap` px free for the Flow ring. A
   *  maximized window has no band. */
  let { gap }: { gap: number } = $props();

  type Direction = 'South' | 'SouthWest' | 'SouthEast';

  function resize(direction: Direction) {
    return (event: MouseEvent) => {
      if (event.button !== 0) return;
      event.preventDefault();
      try {
        getCurrentWindow()
          .startResizeDragging(direction)
          .catch((err: unknown) => console.error('resizing the window failed', err));
      } catch (err) {
        console.error('resizing the window failed', err);
      }
    };
  }
</script>

{#if !windowState.maximized}
  <div class="resize corner sw" data-direction="SouthWest" aria-hidden="true" onmousedown={resize('SouthWest')}></div>
  <div
    class="resize edge"
    data-direction="South"
    aria-hidden="true"
    style="left:var(--corner-zone);right:calc(50% + {gap / 2}px)"
    onmousedown={resize('South')}
  ></div>
  <div
    class="resize edge"
    data-direction="South"
    aria-hidden="true"
    style="left:calc(50% + {gap / 2}px);right:var(--corner-zone)"
    onmousedown={resize('South')}
  ></div>
  <div class="resize corner se" data-direction="SouthEast" aria-hidden="true" onmousedown={resize('SouthEast')}></div>
{/if}

<style>
  .resize {
    --corner-zone: 12px;
    position: absolute;
    bottom: 0;
    z-index: 4;
  }

  .edge {
    height: 5px;
    cursor: ns-resize;
  }

  .corner {
    width: var(--corner-zone);
    height: var(--corner-zone);
  }

  .sw {
    left: 0;
    cursor: nesw-resize;
  }

  .se {
    right: 0;
    cursor: nwse-resize;
  }
</style>
