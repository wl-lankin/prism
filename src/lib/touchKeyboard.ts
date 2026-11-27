/** Logic of the on-screen keyboard for the touchscreen kiosk: which fields count as editable,
 *  the key layouts, and the editing helpers. Kept free of Svelte so it can be tested alone. */

export type Layer = 'letters' | 'symbols';

export type KeyDef =
  | { kind: 'char'; value: string; letter?: boolean; w?: number }
  | { kind: 'shift' | 'backspace' | 'layer' | 'enter' | 'done' | 'space'; w?: number };

export type EditableField = HTMLInputElement | HTMLTextAreaElement;

const TEXT_TYPES = new Set(['text', 'search', 'url', 'email', 'password', 'tel', 'number', '']);

/** Whether focus on this element should bring up the keyboard: a text-like input or a
 *  textarea that the user can actually type into. */
export function isEditable(el: EventTarget | null): el is EditableField {
  if (el instanceof HTMLTextAreaElement) return !el.readOnly && !el.disabled;
  if (el instanceof HTMLInputElement) return TEXT_TYPES.has(el.type) && !el.readOnly && !el.disabled;
  return false;
}

const chars = (s: string, letter = false): KeyDef[] => [...s].map((value) => ({ kind: 'char', value, letter }));

/** The rows of a layer. The two keys beside the space bar depend on what the field takes:
 *  a slash and a dot for addresses, an at sign and a dot for e-mail, otherwise comma and dot. */
export function layoutFor(layer: Layer, fieldType: string): KeyDef[][] {
  const extras: [string, string] = fieldType === 'url' ? ['/', '.'] : fieldType === 'email' ? ['@', '.'] : [',', '.'];
  const bottom: KeyDef[] = [
    { kind: 'layer', w: 1.6 },
    { kind: 'char', value: extras[0] },
    { kind: 'space', w: 4.4 },
    { kind: 'char', value: extras[1] },
    { kind: 'enter', w: 1.6 },
    { kind: 'done', w: 1.6 },
  ];
  if (layer === 'symbols') {
    return [
      chars('1234567890'),
      chars('@.,-_/:;!?'),
      chars('&=+#*\'"()%'),
      [...chars('$€~'), { kind: 'backspace', w: 1.5 }],
      bottom,
    ];
  }
  return [
    chars('qwertzuiopü', true),
    [...chars('asdfghjklöä', true)],
    // ß is no letter here: shifted, toUpperCase would type "SS", two characters for one key.
    [{ kind: 'shift', w: 1.5 }, ...chars('yxcvbnm', true), ...chars('ß'), { kind: 'backspace', w: 1.5 }],
    bottom,
  ];
}

/** Whether the field offers the selection API (an e-mail or number input does not). */
function hasSelection(field: EditableField): boolean {
  try {
    return field.selectionStart !== null && field.selectionEnd !== null;
  } catch {
    return false;
  }
}

function fire(field: EditableField, inputType: string, data: string | null): void {
  field.dispatchEvent(new InputEvent('input', { bubbles: true, inputType, data }));
}

function exec(command: string, value?: string): boolean {
  try {
    return typeof document.execCommand === 'function' && document.execCommand(command, false, value);
  } catch {
    return false;
  }
}

/** Types text at the caret, replacing a selection. execCommand goes first because WebKit then
 *  fires a real input event and keeps the undo stack; the manual path serves engines or field
 *  types where it is not available. */
export function insertText(field: EditableField, text: string): void {
  if (exec('insertText', text)) return;
  if (hasSelection(field)) {
    field.setRangeText(text, field.selectionStart!, field.selectionEnd!, 'end');
  } else {
    field.value += text;
  }
  fire(field, 'insertText', text);
}

/** Deletes the selection, or the character before the caret. */
export function deleteBackward(field: EditableField): void {
  if (exec('delete')) return;
  if (hasSelection(field)) {
    const start = field.selectionStart!;
    const end = field.selectionEnd!;
    if (start === end) {
      if (start === 0) return;
      field.setRangeText('', start - 1, end, 'end');
    } else {
      field.setRangeText('', start, end, 'end');
    }
  } else {
    if (!field.value) return;
    field.value = field.value.slice(0, -1);
  }
  fire(field, 'deleteContentBackward', null);
}

/** The Enter key. Fields listen for keydown Enter (the search palette does), so that event is
 *  sent first; a form is submitted only when nobody handled it. A textarea gets a new line. */
export function pressEnter(field: EditableField): void {
  if (field instanceof HTMLTextAreaElement) {
    insertText(field, '\n');
    return;
  }
  const init = { key: 'Enter', code: 'Enter', bubbles: true, cancelable: true };
  const handled = !field.dispatchEvent(new KeyboardEvent('keydown', init));
  field.dispatchEvent(new KeyboardEvent('keyup', init));
  if (!handled) field.form?.requestSubmit();
}
