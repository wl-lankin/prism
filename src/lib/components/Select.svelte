<script lang="ts" module>
  export interface SelectOption {
    value: string;
    label: string;
    /** A quieter second line, e.g. "not connected". */
    note?: string;
  }

  let ids = 0;

  /** Tallest the list gets before it scrolls. */
  const MAX_LIST = 300;
  const GAP = 6;
  const MARGIN = 8;

  /** The box the list must stay inside: the nearest ancestor that clips (a scroller), or
   *  the window. */
  export function clipBox(element: Element): { top: number; bottom: number } {
    for (let node = element.parentElement; node; node = node.parentElement) {
      const overflow = getComputedStyle(node).overflowY;
      if (overflow === 'auto' || overflow === 'scroll' || overflow === 'hidden' || overflow === 'clip') {
        const rect = node.getBoundingClientRect();
        return { top: rect.top, bottom: rect.bottom };
      }
    }
    return { top: 0, bottom: window.innerHeight };
  }

  /** Where the list goes: below when it fits there, otherwise on the side with more room,
   *  capped to that room. */
  export function placeList(
    trigger: { top: number; bottom: number },
    clip: { top: number; bottom: number },
    needed: number,
  ): { upward: boolean; room: number } {
    const below = clip.bottom - trigger.bottom - GAP - MARGIN;
    const above = trigger.top - clip.top - GAP - MARGIN;
    const upward = needed > below && above > below;
    return { upward, room: Math.max(80, Math.min(MAX_LIST, upward ? above : below)) };
  }
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import Icon from './Icon.svelte';

  /** A select-only combobox (WAI-ARIA pattern) drawn in the Prism style: the button keeps
   *  focus, the list follows with `aria-activedescendant`. Keys: Enter, Space, Alt+Down and
   *  the arrows open it; arrows, Home, End and typing a letter move; Enter or Space picks;
   *  Esc and Tab close. Home and End also open it on the first or last option; letters typed
   *  within half a second match as one prefix. */
  let {
    value,
    options,
    label,
    disabled = false,
    onchange,
  }: {
    value: string;
    options: SelectOption[];
    /** The accessible name (the visible label lives next to it in the row). */
    label: string;
    disabled?: boolean;
    onchange?: (value: string) => void;
  } = $props();

  const id = `select-${++ids}`;
  let open = $state(false);
  let active = $state(0);
  let upward = $state(false);
  /** Room for the list inside the area that clips it, in px. */
  let room = $state(MAX_LIST);
  let button: HTMLButtonElement | undefined = $state();
  let list: HTMLUListElement | undefined = $state();

  const selectedIndex = $derived(Math.max(0, options.findIndex((option) => option.value === value)));
  const selected = $derived(options[selectedIndex]);

  /** Opens the list with `start` (the selected option by default) active. */
  async function show(start = selectedIndex) {
    if (disabled || open || options.length === 0) return;
    active = Math.max(0, Math.min(options.length - 1, start));
    // Stay inside whatever clips the list (the settings scroller), flipping up if needed.
    if (button) {
      const placed = placeList(button.getBoundingClientRect(), clipBox(button), Math.min(options.length * 40 + 12, MAX_LIST));
      upward = placed.upward;
      room = placed.room;
    }
    open = true;
    await tick();
    list?.querySelector(`#${id}-${active}`)?.scrollIntoView?.({ block: 'nearest' });
  }

  function hide() {
    open = false;
  }

  function choose(index: number) {
    const option = options[index];
    hide();
    button?.focus();
    if (option && option.value !== value) onchange?.(option.value);
  }

  function move(index: number) {
    active = Math.max(0, Math.min(options.length - 1, index));
    tick().then(() => list?.querySelector(`#${id}-${active}`)?.scrollIntoView?.({ block: 'nearest' }));
  }

  /** Letters typed within `TYPEAHEAD_MS` of each other form one prefix. */
  const TYPEAHEAD_MS = 500;
  let typed = '';
  let typedAt = 0;

  /** Jumps to the option starting with what was typed: a single letter moves on to the next
   *  match after the active one, a longer prefix may stay on the active one. */
  function typeahead(letter: string) {
    const now = Date.now();
    typed = now - typedAt <= TYPEAHEAD_MS ? typed + letter.toLowerCase() : letter.toLowerCase();
    typedAt = now;
    // The same letter again cycles through its matches.
    const search = /^(.)\1+$/.test(typed) ? typed[0] : typed;
    const first = search.length > 1 ? 0 : 1;
    for (let step = first; step < options.length + first; step++) {
      const index = (active + step) % options.length;
      if (options[index].label.toLowerCase().startsWith(search)) {
        move(index);
        return;
      }
    }
  }

  function onkeydown(event: KeyboardEvent) {
    if (disabled) return;
    if (!open) {
      if (['ArrowDown', 'ArrowUp', 'Enter', ' ', 'Home', 'End'].includes(event.key)) {
        event.preventDefault();
        const start =
          event.key === 'Home' ? 0 : event.key === 'End' ? options.length - 1 : event.key === 'ArrowUp' && !event.altKey ? selectedIndex - 1 : selectedIndex;
        void show(start);
      }
      return;
    }
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        if (event.altKey) choose(active);
        else move(active + 1);
        break;
      case 'ArrowUp':
        event.preventDefault();
        if (event.altKey) choose(active);
        else move(active - 1);
        break;
      case 'Home':
      case 'PageUp':
        event.preventDefault();
        move(0);
        break;
      case 'End':
      case 'PageDown':
        event.preventDefault();
        move(options.length - 1);
        break;
      case 'Enter':
      case ' ':
        event.preventDefault();
        choose(active);
        break;
      case 'Escape':
        // Only the list closes, not a dialog around it.
        event.preventDefault();
        event.stopPropagation();
        hide();
        break;
      case 'Tab':
        hide();
        break;
      default:
        if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) typeahead(event.key);
    }
  }

  function onwindowpointerdown(event: PointerEvent) {
    if (!open) return;
    const target = event.target as Node | null;
    if (target && (button?.contains(target) || list?.contains(target))) return;
    hide();
  }
</script>

<svelte:window onpointerdown={onwindowpointerdown} onblur={hide} />

<div class="select" class:open>
  <button
    bind:this={button}
    type="button"
    class="trigger"
    role="combobox"
    aria-label={label}
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-controls={`${id}-list`}
    aria-activedescendant={open ? `${id}-${active}` : undefined}
    {disabled}
    onclick={() => (open ? hide() : show())}
    {onkeydown}
    onblur={(event) => {
      // Clicking an option keeps focus (pointerdown is prevented); anything else closes.
      if (!list?.contains(event.relatedTarget as Node | null)) hide();
    }}
  >
    <span class="value">{selected?.label ?? ''}</span>
    {#if selected?.note}<span class="note">{selected.note}</span>{/if}
    <span class="chevron"><Icon name="chevronDown" size={16} /></span>
  </button>

  {#if open}
    <ul
      bind:this={list}
      id={`${id}-list`}
      class="list"
      class:upward
      style="max-height:{room}px"
      role="listbox"
      aria-label={label}
      tabindex="-1"
      onpointerdown={(event) => event.preventDefault()}
    >
      {#each options as option, index (option.value)}
        <li
          id={`${id}-${index}`}
          role="option"
          class="option"
          class:active={index === active}
          aria-selected={option.value === value}
          onpointermove={() => (active = index)}
          onclick={() => choose(index)}
          onkeydown={() => {}}
        >
          <span class="mark">{#if option.value === value}<Icon name="check" size={16} />{/if}</span>
          <span class="text">
            <span class="label">{option.label}</span>
            {#if option.note}<span class="note">{option.note}</span>{/if}
          </span>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .select {
    position: relative;
    width: 100%;
  }

  .trigger {
    width: 100%;
    height: 36px;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 8px 0 12px;
    border: 0;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.055);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
    color: var(--text);
    font: inherit;
    font-size: 13.5px;
    text-align: left;
    cursor: pointer;
    transition:
      background-color var(--dur) ease-out,
      box-shadow var(--dur) ease-out;
  }

  .trigger:hover:not(:disabled),
  .open .trigger {
    background: rgba(255, 255, 255, 0.085);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.12);
  }

  .trigger:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
  }

  .trigger:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .value {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .note {
    flex: none;
    font-size: 12px;
    color: var(--text-dim);
  }

  .chevron {
    margin-left: auto;
    flex: none;
    display: grid;
    color: var(--text-dim);
    transition: transform var(--dur) ease-out;
  }

  .open .chevron {
    transform: rotate(180deg);
  }

  .list {
    position: absolute;
    z-index: 10;
    top: calc(100% + 6px);
    left: 0;
    right: 0;
    box-sizing: border-box;
    overflow-y: auto;
    margin: 0;
    padding: 5px;
    list-style: none;
    border-radius: 12px;
    background: var(--raised);
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.09),
      0 18px 40px -8px rgba(0, 0, 0, 0.7),
      0 4px 12px rgba(0, 0, 0, 0.35);
    animation: drop var(--dur) ease-out;
    scrollbar-color: auto;
  }

  .list.upward {
    top: auto;
    bottom: calc(100% + 6px);
  }

  .list:focus {
    outline: none;
  }

  .option {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 34px;
    padding: 6px 10px 6px 6px;
    border-radius: 8px;
    font-size: 13.5px;
    cursor: pointer;
  }

  .option.active {
    background: rgba(255, 255, 255, 0.08);
  }

  .mark {
    width: 16px;
    flex: none;
    display: grid;
    color: var(--spec-3);
  }

  .text {
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  @keyframes drop {
    from {
      opacity: 0;
      transform: translateY(-3px);
    }
  }
</style>
