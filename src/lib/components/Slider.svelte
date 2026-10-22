<script lang="ts">
  import { ring } from '../stores/ring.svelte';

  let {
    value,
    label,
    valueText,
    step = 0.05,
    spectrum = false,
    disabled = false,
    oninput,
    oncommit,
    oncancel,
  }: {
    /** 0..1 */
    value: number;
    label: string;
    valueText?: string;
    step?: number;
    /** Fill with the cover colors of the Flow ring (the spectrum without a cover) instead of plain white. */
    spectrum?: boolean;
    disabled?: boolean;
    /** Every change while dragging or on a key press. */
    oninput?: (value: number) => void;
    /** The final value when the pointer is released or a key is pressed. */
    oncommit?: (value: number) => void;
    /** A drag ended without a commit (pointer cancelled or capture lost). */
    oncancel?: () => void;
  } = $props();

  let el: HTMLDivElement;
  let dragging = $state(false);
  let dragValue = $state(0);

  const spectrumStyle = $derived(
    spectrum ? ring.colors.map((color, i) => `--seek-${i + 1}:${color}`).join(';') : undefined,
  );

  const shown = $derived(Math.min(Math.max(dragging ? dragValue : value || 0, 0), 1));

  function at(event: PointerEvent): number {
    const rect = el.getBoundingClientRect();
    return rect.width ? Math.min(Math.max((event.clientX - rect.left) / rect.width, 0), 1) : 0;
  }

  function down(event: PointerEvent) {
    if (disabled || event.button !== 0) return;
    el.setPointerCapture(event.pointerId);
    dragging = true;
    dragValue = at(event);
    oninput?.(dragValue);
  }

  function move(event: PointerEvent) {
    if (!dragging) return;
    dragValue = at(event);
    oninput?.(dragValue);
  }

  function up(event: PointerEvent) {
    if (!dragging) return;
    dragValue = at(event);
    dragging = false;
    oncommit?.(dragValue);
  }

  // `up` clears `dragging` first, so a capture lost after a normal release does nothing here.
  function cancel() {
    if (!dragging) return;
    dragging = false;
    oncancel?.();
  }

  function key(event: KeyboardEvent) {
    if (disabled) return;
    const deltas: Record<string, number> = { ArrowRight: step, ArrowUp: step, ArrowLeft: -step, ArrowDown: -step };
    let next: number;
    if (event.key in deltas) next = shown + deltas[event.key];
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = 1;
    else return;
    event.preventDefault();
    next = Math.min(Math.max(next, 0), 1);
    oninput?.(next);
    oncommit?.(next);
  }
</script>

<div
  bind:this={el}
  class="slider"
  class:dragging
  class:disabled
  role="slider"
  tabindex={disabled ? -1 : 0}
  aria-label={label}
  aria-valuemin={0}
  aria-valuemax={100}
  aria-valuenow={Math.round(shown * 100)}
  aria-valuetext={valueText}
  aria-disabled={disabled}
  style="--v:{shown * 100}%"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={cancel}
  onlostpointercapture={cancel}
  onkeydown={key}
>
  <div class="rail">
    <div class="fill" class:spectrum style={spectrumStyle}></div>
  </div>
  <div class="thumb"></div>
</div>

<style>
  /* Registered so they interpolate: a new cover's colors fade in instead of jumping. */
  @property --seek-1 {
    syntax: '<color>';
    inherits: false;
    initial-value: #ff4d6d;
  }

  @property --seek-2 {
    syntax: '<color>';
    inherits: false;
    initial-value: #ffc94d;
  }

  @property --seek-3 {
    syntax: '<color>';
    inherits: false;
    initial-value: #4dffb2;
  }

  @property --seek-4 {
    syntax: '<color>';
    inherits: false;
    initial-value: #4da6ff;
  }

  .slider {
    position: relative;
    height: 16px;
    display: flex;
    align-items: center;
    cursor: pointer;
    touch-action: none;
    outline: none;
  }

  .slider.disabled {
    cursor: default;
    opacity: 0.5;
  }

  .rail {
    position: relative;
    width: 100%;
    height: 3px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.1);
    overflow: hidden;
    transition: height var(--dur) ease-out;
  }

  .slider:hover:not(.disabled) .rail,
  .slider.dragging .rail,
  .slider:focus-visible .rail {
    height: 5px;
  }

  .fill {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    background: rgba(243, 243, 245, 0.78);
    clip-path: inset(0 calc(100% - var(--v)) 0 0 round 3px);
  }

  /* The gradient spans the whole rail, so a color always marks the same spot in the track. */
  .fill.spectrum {
    background: linear-gradient(90deg, var(--seek-1), var(--seek-2), var(--seek-3), var(--seek-4));
    /* The same 600 ms fade as the ring's colors, also under reduced motion. */
    transition:
      --seek-1 600ms ease,
      --seek-2 600ms ease,
      --seek-3 600ms ease,
      --seek-4 600ms ease;
  }

  .thumb {
    position: absolute;
    left: var(--v);
    top: 50%;
    width: 11px;
    height: 11px;
    margin: -5.5px 0 0 -5.5px;
    border-radius: 50%;
    background: var(--text);
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.5);
    opacity: 0;
    transform: scale(0.6);
    transition:
      opacity var(--dur) ease-out,
      transform var(--dur) ease-out;
  }

  .slider:hover:not(.disabled) .thumb,
  .slider.dragging .thumb,
  .slider:focus-visible .thumb {
    opacity: 1;
    transform: scale(1);
  }

  .slider:focus-visible {
    border-radius: 4px;
    box-shadow: 0 0 0 2px rgba(77, 166, 255, 0.55);
  }
</style>
