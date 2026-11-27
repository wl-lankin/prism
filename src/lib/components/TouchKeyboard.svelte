<script lang="ts">
  import { tick } from 'svelte';
  import { t, type Key } from '../i18n';
  import { windowState } from '../stores/window.svelte';
  import { deleteBackward, insertText, isEditable, layoutFor, pressEnter, type EditableField, type KeyDef, type Layer } from '../touchKeyboard';

  type Shift = 'off' | 'once' | 'lock';

  const FALLBACK_HEIGHT = 250;
  const DOUBLE_TAP_MS = 350;
  const REPEAT_START_MS = 400;
  const REPEAT_EVERY_MS = 60;

  let field = $state<EditableField | null>(null);
  let layer = $state<Layer>('letters');
  let shift = $state<Shift>('off');
  let root = $state<HTMLElement>();
  let lastShiftTap = 0;
  let repeatStart: ReturnType<typeof setTimeout> | undefined;
  let repeatTick: ReturnType<typeof setInterval> | undefined;

  const rows = $derived(layoutFor(layer, field?.type ?? ''));
  const upper = $derived(shift !== 'off');

  function hide() {
    stopRepeat();
    field = null;
  }

  function show(next: EditableField) {
    // A new field starts on the letters with shift off, like a phone keyboard.
    if (next !== field) {
      layer = next.type === 'number' || next.type === 'tel' ? 'symbols' : 'letters';
      shift = 'off';
    }
    field = next;
  }

  // Follow focus only in kiosk mode; elsewhere a real keyboard exists.
  $effect(() => {
    if (!windowState.kiosk) return;
    const onFocusIn = (e: FocusEvent) => (isEditable(e.target) ? show(e.target) : hide());
    // A tap on the bare background blurs the field without focusing anything, so look again
    // once focus has settled.
    const onFocusOut = (e: FocusEvent) => {
      if (e.relatedTarget) return;
      setTimeout(() => {
        if (!isEditable(document.activeElement)) hide();
      }, 0);
    };
    document.addEventListener('focusin', onFocusIn);
    document.addEventListener('focusout', onFocusOut);
    return () => {
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
      hide();
    };
  });

  // Hide when the field is removed from the page (a dialog closes, a view changes).
  $effect(() => {
    if (!field) return;
    const watched = field;
    const observer = new MutationObserver(() => {
      if (!watched.isConnected) hide();
    });
    observer.observe(document.body, { childList: true, subtree: true });
    return () => observer.disconnect();
  });

  // Tell the page how much of its bottom the keyboard covers, and bring the field above it.
  $effect(() => {
    if (!field) return;
    const current = field;
    const style = document.documentElement.style;
    void tick().then(() => {
      if (field !== current) return;
      const height = root?.offsetHeight || FALLBACK_HEIGHT;
      style.setProperty('--osk-h', `${height}px`);
      if (current.getBoundingClientRect().bottom > window.innerHeight - height) {
        current.scrollIntoView?.({ block: 'center' });
      }
    });
    return () => style.removeProperty('--osk-h');
  });

  function stopRepeat() {
    clearTimeout(repeatStart);
    clearInterval(repeatTick);
    repeatStart = repeatTick = undefined;
  }

  function type(text: string) {
    if (!field) return;
    insertText(field, text);
    if (shift === 'once') shift = 'off';
  }

  function pressShift() {
    const now = Date.now();
    if (shift === 'lock') shift = 'off';
    else if (shift === 'once' && now - lastShiftTap < DOUBLE_TAP_MS) shift = 'lock';
    else shift = shift === 'once' ? 'off' : 'once';
    lastShiftTap = now;
  }

  function press(key: KeyDef) {
    if (!field) return;
    switch (key.kind) {
      case 'char':
        type(key.letter && upper ? key.value.toUpperCase() : key.value);
        break;
      case 'space':
        type(' ');
        break;
      case 'shift':
        pressShift();
        break;
      case 'layer':
        layer = layer === 'letters' ? 'symbols' : 'letters';
        break;
      case 'enter':
        pressEnter(field);
        break;
      case 'backspace': {
        const target = field;
        deleteBackward(target);
        stopRepeat();
        repeatStart = setTimeout(() => {
          repeatTick = setInterval(() => deleteBackward(target), REPEAT_EVERY_MS);
        }, REPEAT_START_MS);
        break;
      }
      case 'done': {
        const target = field;
        hide();
        target.blur();
        break;
      }
    }
  }

  const LABELS: Record<string, Key> = {
    space: 'keyboard.space',
    backspace: 'keyboard.backspace',
    shift: 'keyboard.shift',
    enter: 'keyboard.enter',
    done: 'keyboard.done',
  };

  function ariaLabel(key: KeyDef): string | undefined {
    if (key.kind === 'char') return undefined;
    if (key.kind === 'layer') return t(layer === 'letters' ? 'keyboard.symbols' : 'keyboard.letters');
    return t(LABELS[key.kind]);
  }

  function glyph(key: KeyDef): string {
    switch (key.kind) {
      case 'char':
        return key.letter && upper ? key.value.toUpperCase() : key.value;
      case 'shift':
        return '⇧';
      case 'backspace':
        return '⌫';
      case 'enter':
        return '↵';
      case 'layer':
        return layer === 'letters' ? '?123' : 'ABC';
      case 'done':
        return t('keyboard.done');
      default:
        return '';
    }
  }
</script>

{#if field}
  <div class="osk" role="group" aria-label={t('keyboard.label')} bind:this={root}>
    {#each rows as row, r (r)}
      <div class="row">
        {#each row as key, k (k)}
          <button
            type="button"
            tabindex="-1"
            class="key"
            class:special={key.kind !== 'char' && key.kind !== 'space'}
            class:on={key.kind === 'shift' && shift === 'once'}
            class:lock={key.kind === 'shift' && shift === 'lock'}
            style:flex={key.w ?? 1}
            aria-label={ariaLabel(key)}
            aria-pressed={key.kind === 'shift' ? shift !== 'off' : undefined}
            onmousedown={(e) => e.preventDefault()}
            onpointerdown={(e) => {
              e.preventDefault();
              press(key);
            }}
            onpointerup={stopRepeat}
            onpointercancel={stopRepeat}
            onpointerleave={stopRepeat}
          >
            {glyph(key)}
          </button>
        {/each}
      </div>
    {/each}
  </div>
{/if}

<style>
  /* Solid and effect free: the Pi's GPU pays a full screen copy for every blur or shadow.
     Above the search palette (20) and the context menus (40). */
  .osk {
    position: fixed;
    inset: auto 0 0 0;
    z-index: 100;
    height: 250px;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px;
    background: var(--bg);
    border-top: 1px solid var(--line);
    font-family: var(--font-ui);
    user-select: none;
    -webkit-user-select: none;
    touch-action: manipulation;
  }

  .row {
    flex: 1;
    min-height: 44px;
    display: flex;
    gap: 4px;
  }

  .key {
    flex: 1;
    min-width: 0;
    padding: 0;
    border: 0;
    border-radius: var(--radius-s);
    background: var(--raised);
    color: var(--text);
    font: inherit;
    font-size: 20px;
    cursor: default;
    -webkit-tap-highlight-color: transparent;
  }

  .key:active {
    background: #2e2e37;
  }

  .key.special {
    background: var(--surface);
    color: var(--text-dim);
    font-size: 17px;
  }

  .key.special:active {
    background: var(--raised);
  }

  .key.on {
    color: var(--text);
    background: var(--raised);
  }

  .key.lock {
    color: var(--bg);
    background: var(--text);
  }
</style>
