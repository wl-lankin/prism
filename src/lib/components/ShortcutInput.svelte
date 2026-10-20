<script lang="ts" module>
  /** The backend's default (`Settings::default`). */
  export const DEFAULT_SHORTCUT = 'CommandOrControl+Alt+P';

  export const IS_MAC = typeof navigator !== 'undefined' && /mac/i.test(navigator.platform || navigator.userAgent || '');

  const MODIFIER_KEYS = new Set(['Control', 'Alt', 'AltGraph', 'Shift', 'Meta', 'OS', 'Super', 'Hyper', 'Fn']);

  /** Short names for `KeyboardEvent.code` values the global shortcut parser also reads. */
  const CODE_NAMES: Record<string, string> = {
    ArrowUp: 'Up',
    ArrowDown: 'Down',
    ArrowLeft: 'Left',
    ArrowRight: 'Right',
  };

  /** The accelerator for a key press, as the backend parses it, or null while the press
   *  is not a shortcut yet (a lone modifier, or no Ctrl, Alt or Cmd). On macOS Cmd is the
   *  CommandOrControl key and Ctrl stays Control; elsewhere Ctrl is it and the Windows key is
   *  Super. */
  export function toAccelerator(event: KeyboardEvent, mac: boolean): string | null {
    if (MODIFIER_KEYS.has(event.key) || !event.code) return null;
    if (!event.ctrlKey && !event.altKey && !event.metaKey) return null;
    const parts: string[] = [];
    if (mac) {
      if (event.metaKey) parts.push('CommandOrControl');
      if (event.ctrlKey) parts.push('Control');
    } else {
      if (event.ctrlKey) parts.push('CommandOrControl');
      if (event.metaKey) parts.push('Super');
    }
    if (event.altKey) parts.push('Alt');
    if (event.shiftKey) parts.push('Shift');
    const code = event.code;
    let key = CODE_NAMES[code] ?? code;
    if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
    else if (/^Digit\d$/.test(code)) key = code.slice(5);
    parts.push(key);
    return parts.join('+');
  }

  /** An accelerator in the platform's words: Ctrl+Alt+P, Cmd+Option+P. */
  export function describeShortcut(accelerator: string, mac: boolean): string {
    const words: Record<string, string> = mac
      ? { commandorcontrol: 'Cmd', cmdorctrl: 'Cmd', command: 'Cmd', cmd: 'Cmd', super: 'Cmd', control: 'Ctrl', ctrl: 'Ctrl', alt: 'Option', option: 'Option', shift: 'Shift' }
      : { commandorcontrol: 'Ctrl', cmdorctrl: 'Ctrl', control: 'Ctrl', ctrl: 'Ctrl', super: 'Win', command: 'Win', cmd: 'Win', alt: 'Alt', option: 'Alt', shift: 'Shift' };
    return accelerator
      .split('+')
      .filter(Boolean)
      .map((part) => words[part.toLowerCase()] ?? part)
      .join('+');
  }
</script>

<script lang="ts">
  import { t } from '../i18n';

  /** Records a global shortcut: click (or Enter), then press the keys. It needs Ctrl, Alt or
   *  Cmd plus one other key; Esc cancels, Backspace goes back to the default. */
  let { value, onchange }: { value: string; onchange: (accelerator: string) => void } = $props();

  let recording = $state(false);
  const shown = $derived(describeShortcut(value, IS_MAC));

  function onkeydown(event: KeyboardEvent) {
    if (!recording) return;
    // Tab still moves on (and ends the recording on blur).
    if (event.key === 'Tab' && !event.ctrlKey && !event.altKey && !event.metaKey) return;
    // While recording, keys belong here, not to the app's own shortcuts.
    event.preventDefault();
    event.stopPropagation();
    if (event.key === 'Escape') {
      recording = false;
      return;
    }
    if (event.key === 'Backspace' && !event.ctrlKey && !event.altKey && !event.metaKey) {
      recording = false;
      if (value !== DEFAULT_SHORTCUT) onchange(DEFAULT_SHORTCUT);
      return;
    }
    const accelerator = toAccelerator(event, IS_MAC);
    if (!accelerator) return;
    recording = false;
    if (accelerator !== value) onchange(accelerator);
  }
</script>

<button
  type="button"
  class="shortcut"
  class:recording
  aria-pressed={recording}
  aria-label={recording ? t('settings.shortcut.record') : t('settings.shortcut.label', { keys: shown })}
  onclick={() => (recording = !recording)}
  onblur={() => (recording = false)}
  {onkeydown}
>
  {#if recording}
    <span class="prompt">{t('settings.shortcut.record')}</span>
  {:else}
    {#each shown.split('+') as part, index (index)}
      {#if index > 0}<span class="plus" aria-hidden="true">+</span>{/if}<kbd>{part}</kbd>
    {/each}
  {/if}
</button>

<style>
  .shortcut {
    width: 100%;
    height: 36px;
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 0 12px;
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

  .shortcut:hover {
    background: rgba(255, 255, 255, 0.085);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.12);
  }

  .shortcut.recording {
    box-shadow: inset 0 0 0 1px var(--spec-4);
  }

  .shortcut:focus-visible {
    outline: 2px solid var(--spec-4);
    outline-offset: 2px;
  }

  kbd {
    min-width: 22px;
    height: 24px;
    display: inline-grid;
    place-items: center;
    padding: 0 7px;
    border-radius: 6px;
    background: var(--raised);
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.1),
      0 1px 0 rgba(0, 0, 0, 0.4);
    font: inherit;
    font-size: 12.5px;
    font-weight: 550;
  }

  .plus {
    font-size: 12px;
    color: var(--text-dim);
  }

  .prompt {
    color: var(--text-dim);
  }
</style>
