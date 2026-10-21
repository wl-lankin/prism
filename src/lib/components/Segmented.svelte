<script lang="ts" generics="T extends string">
  import { onMount, tick } from 'svelte';

  let {
    options,
    value,
    labelledby,
    attr,
    onpick,
  }: {
    options: { value: T; label: string }[];
    value: T;
    /** The id of the element that names the group. */
    labelledby: string;
    /** The data attribute that carries each option's value on its button. */
    attr: string;
    onpick: (value: T) => void;
  } = $props();

  let group: HTMLDivElement;
  let x = $state(0);
  let width = $state(0);
  /** Off until the first measurement is painted, so the marker does not slide in from nowhere. */
  let ready = $state(false);

  function measure() {
    const on = group?.querySelector<HTMLElement>('[aria-checked="true"]');
    if (!on) return;
    x = on.offsetLeft;
    width = on.offsetWidth;
  }

  $effect(() => {
    void value;
    void options;
    tick().then(measure);
  });

  onMount(() => {
    measure();
    const frame = requestAnimationFrame(() => (ready = true));
    // Option widths change with the language.
    const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(measure) : undefined;
    observer?.observe(group);
    for (const button of group.querySelectorAll('button')) observer?.observe(button);
    return () => {
      cancelAnimationFrame(frame);
      observer?.disconnect();
    };
  });

  /** Arrow keys: pick the neighbour (wrapping) and focus it. */
  function onkey(event: KeyboardEvent, index: number) {
    const step = event.key === 'ArrowRight' || event.key === 'ArrowDown' ? 1 : event.key === 'ArrowLeft' || event.key === 'ArrowUp' ? -1 : 0;
    if (!step) return;
    event.preventDefault();
    const next = options[(index + step + options.length) % options.length].value;
    onpick(next);
    queueMicrotask(() => group.querySelector<HTMLElement>(`[${attr}="${next}"]`)?.focus());
  }
</script>

<div class="segments" role="radiogroup" aria-labelledby={labelledby} bind:this={group} style="--cols:{options.length}">
  <span class="marker" class:ready aria-hidden="true" style="width:{width}px;transform:translateX({x}px)"></span>
  {#each options as option, index (option.value)}
    {@const on = value === option.value}
    <button
      type="button"
      role="radio"
      {...{ [attr]: option.value }}
      aria-checked={on}
      tabindex={on ? 0 : -1}
      class:on
      onclick={() => onpick(option.value)}
      onkeydown={(event) => onkey(event, index)}
    >
      {option.label}
    </button>
  {/each}
</div>

<style>
  .segments {
    position: relative;
    display: grid;
    grid-template-columns: repeat(var(--cols), 1fr);
    gap: 2px;
    padding: 3px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.055);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
  }

  /* One marker, sized and placed on the active option. Its slide is small motion and stays
     under reduced motion on purpose. */
  .marker {
    position: absolute;
    left: 0;
    top: 3px;
    bottom: 3px;
    border-radius: 7px;
    background: var(--raised);
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.1),
      0 2px 6px rgba(0, 0, 0, 0.35);
    pointer-events: none;
  }

  .marker.ready {
    transition:
      transform 220ms cubic-bezier(0.22, 1, 0.36, 1),
      width 220ms cubic-bezier(0.22, 1, 0.36, 1);
  }

  button {
    position: relative;
    height: 30px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--text-dim);
    font: inherit;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: color var(--dur) ease-out;
  }

  button:hover:not(.on) {
    color: var(--text);
  }

  button.on {
    color: var(--text);
  }

  button:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 1px;
  }
</style>
