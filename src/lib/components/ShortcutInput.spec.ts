import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

const { default: ShortcutInput, DEFAULT_SHORTCUT, describeShortcut } = await import('./ShortcutInput.svelte');
const { setLang } = await import('../i18n');

let target: HTMLElement;
const mounted: ReturnType<typeof mount>[] = [];

beforeEach(() => {
  setLang('en');
  target = document.createElement('div');
  document.body.append(target);
});

afterEach(() => {
  for (const app of mounted.splice(0)) unmount(app);
  target.remove();
});

function press(element: Element, key: string, code: string, init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent('keydown', { key, code, bubbles: true, cancelable: true, ...init });
  element.dispatchEvent(event);
  flushSync();
  return event;
}

function setup(value = 'CommandOrControl+Alt+P') {
  const onchange = vi.fn();
  mounted.push(mount(ShortcutInput, { target, props: { value, onchange } }));
  flushSync();
  const button = target.querySelector<HTMLButtonElement>('button')!;
  return { button, onchange };
}

function record(button: HTMLButtonElement) {
  button.click();
  flushSync();
}

it('records Ctrl+Alt+P as a CommandOrControl accelerator', () => {
  const { button, onchange } = setup('CommandOrControl+Alt+M');
  expect(button.textContent).toContain('Ctrl+Alt+M');
  record(button);
  expect(button.textContent).toContain('Press the keys');
  const event = press(button, 'p', 'KeyP', { ctrlKey: true, altKey: true });
  expect(event.defaultPrevented).toBe(true);
  expect(onchange).toHaveBeenCalledWith('CommandOrControl+Alt+P');
  expect(button.getAttribute('aria-pressed')).toBe('false');
});

it('ignores a lone modifier and a key without Ctrl, Alt or Cmd', () => {
  const { button, onchange } = setup();
  record(button);
  press(button, 'Control', 'ControlLeft', { ctrlKey: true });
  press(button, 'Alt', 'AltLeft', { altKey: true });
  press(button, 'p', 'KeyP');
  press(button, 'P', 'KeyP', { shiftKey: true });
  expect(onchange).not.toHaveBeenCalled();
  // Still recording.
  expect(button.getAttribute('aria-pressed')).toBe('true');
});

it('restores the shown shortcut on Esc and resets to the default on Backspace', () => {
  const { button, onchange } = setup('CommandOrControl+Shift+F9');
  record(button);
  const outer = vi.fn();
  window.addEventListener('keydown', outer);
  press(button, 'Escape', 'Escape');
  window.removeEventListener('keydown', outer);
  expect(outer).not.toHaveBeenCalled();
  expect(onchange).not.toHaveBeenCalled();
  expect(button.getAttribute('aria-pressed')).toBe('false');
  expect(button.textContent).toContain('Ctrl+Shift+F9');

  record(button);
  press(button, 'Backspace', 'Backspace');
  expect(onchange).toHaveBeenCalledWith(DEFAULT_SHORTCUT);
});

it('keeps keys from the rest of the app while recording', () => {
  const { button } = setup();
  record(button);
  const outer = vi.fn();
  window.addEventListener('keydown', outer);
  press(button, 'k', 'KeyK', { ctrlKey: true });
  window.removeEventListener('keydown', outer);
  expect(outer).not.toHaveBeenCalled();
});

it('names keys in platform words', () => {
  expect(describeShortcut('CommandOrControl+Alt+P', false)).toBe('Ctrl+Alt+P');
  expect(describeShortcut('CommandOrControl+Alt+P', true)).toBe('Cmd+Option+P');
  expect(describeShortcut('Control+Shift+Up', true)).toBe('Ctrl+Shift+Up');
  expect(describeShortcut('Super+Alt+1', false)).toBe('Win+Alt+1');
  expect(describeShortcut('CommandOrControl+Alt+Minus', false)).toBe('Ctrl+Alt+-');
  expect(describeShortcut('CommandOrControl+BracketLeft', true)).toBe('Cmd+[');
  expect(describeShortcut('CommandOrControl+Alt+Numpad7', false)).toBe('Ctrl+Alt+Num 7');
  expect(describeShortcut('CommandOrControl+Alt+PageDown', false)).toBe('Ctrl+Alt+Page Down');
});

it('stores code names the shortcut parser reads and shows them as key caps', () => {
  const { button, onchange } = setup();
  record(button);
  press(button, '+', 'NumpadAdd', { ctrlKey: true, altKey: true });
  expect(onchange).toHaveBeenCalledWith('CommandOrControl+Alt+NumpadAdd');
  unmount(mounted.pop()!);
  setup('CommandOrControl+Alt+NumpadAdd');
  const caps = [...target.querySelectorAll('kbd')].map((kbd) => kbd.textContent);
  expect(caps).toEqual(['Ctrl', 'Alt', 'Num +']);
});
