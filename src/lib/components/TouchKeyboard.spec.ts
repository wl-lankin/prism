import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({}) }));

const { windowState } = await import('../stores/window.svelte');
const { default: TouchKeyboard } = await import('./TouchKeyboard.svelte');

let app: object | undefined;

async function settle() {
  flushSync();
  await tick();
  await tick();
  flushSync();
}

async function setup(kiosk = true) {
  windowState.kiosk = kiosk;
  app = mount(TouchKeyboard, { target: document.body });
  await settle();
}

function field(html: string): HTMLInputElement {
  const host = document.createElement('div');
  host.innerHTML = html;
  document.body.append(host);
  return host.querySelector('input,textarea') as HTMLInputElement;
}

const keyboard = () => document.querySelector<HTMLElement>('.osk');
const key = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>('.osk .key')].find((b) => (b.getAttribute('aria-label') ?? b.textContent!.trim()) === label || b.textContent!.trim() === label)!;

async function tap(label: string) {
  key(label).dispatchEvent(new Event('pointerdown', { bubbles: true, cancelable: true }));
  await settle();
}

async function focus(el: HTMLElement) {
  el.focus();
  await settle();
}

beforeEach(() => {
  document.body.innerHTML = '';
  document.documentElement.style.removeProperty('--osk-h');
});

afterEach(async () => {
  if (app) unmount(app);
  app = undefined;
  windowState.kiosk = false;
  vi.useRealTimers();
});

it('shows for a text input, only in kiosk mode', async () => {
  await setup(false);
  const input = field('<input type="text">');
  await focus(input);
  expect(keyboard()).toBeNull();
  unmount(app!);
  document.body.innerHTML = '';
  await setup(true);
  const again = field('<input type="text">');
  await focus(again);
  expect(keyboard()).not.toBeNull();
  expect(keyboard()!.getAttribute('role')).toBe('group');
});

it('ignores readonly fields, checkboxes and buttons', async () => {
  await setup();
  await focus(field('<input type="text" readonly>'));
  expect(keyboard()).toBeNull();
  await focus(field('<input type="checkbox">'));
  expect(keyboard()).toBeNull();
});

it('hides when focus moves to something that is not editable', async () => {
  await setup();
  const input = field('<input type="text">');
  const button = document.createElement('button');
  document.body.append(button);
  await focus(input);
  expect(keyboard()).not.toBeNull();
  await focus(button);
  expect(keyboard()).toBeNull();
});

it('hides when the field leaves the page', async () => {
  await setup();
  const input = field('<input type="text">');
  await focus(input);
  input.parentElement!.remove();
  await new Promise((r) => setTimeout(r, 0));
  await settle();
  expect(keyboard()).toBeNull();
});

it('types a letter at the caret and fires an input event', async () => {
  await setup();
  const input = field('<input type="text" value="ac">');
  const seen: string[] = [];
  input.addEventListener('input', () => seen.push(input.value));
  await focus(input);
  input.setSelectionRange(1, 1);
  await tap('b');
  expect(input.value).toBe('abc');
  expect(seen).toEqual(['abc']);
  expect(input.selectionStart).toBe(2);
});

it('appends in a field without a selection API', async () => {
  await setup();
  const input = field('<input type="email" value="a">');
  await focus(input);
  await tap('b');
  expect(input.value).toBe('ab');
});

it('shift is one-shot, a double tap locks caps', async () => {
  await setup();
  const input = field('<input type="text">');
  await focus(input);
  await tap('Shift');
  await tap('A');
  await tap('a');
  expect(input.value).toBe('Aa');
  await tap('Shift');
  await tap('Shift');
  await tap('B');
  await tap('C');
  expect(input.value).toBe('AaBC');
  await tap('Shift');
  await tap('d');
  expect(input.value).toBe('AaBCd');
});

it('backspace removes the char before the caret and the selection', async () => {
  await setup();
  const input = field('<input type="text" value="hello">');
  await focus(input);
  input.setSelectionRange(3, 3);
  await tap('Delete');
  expect(input.value).toBe('helo');
  input.setSelectionRange(0, 2);
  await tap('Delete');
  expect(input.value).toBe('lo');
});

it('backspace repeats while held and stops on release', async () => {
  await setup();
  const input = field('<input type="text" value="abcdefghij">');
  await focus(input);
  input.setSelectionRange(10, 10);
  vi.useFakeTimers();
  key('Delete').dispatchEvent(new Event('pointerdown', { bubbles: true, cancelable: true }));
  expect(input.value).toBe('abcdefghi');
  vi.advanceTimersByTime(400 + 60 * 3);
  expect(input.value).toBe('abcdef');
  key('Delete').dispatchEvent(new Event('pointerup', { bubbles: true }));
  vi.advanceTimersByTime(500);
  expect(input.value).toBe('abcdef');
});

it('Enter sends keydown Enter to the field', async () => {
  await setup();
  const input = field('<input type="text">');
  const keys: string[] = [];
  input.addEventListener('keydown', (e) => keys.push(e.key));
  await focus(input);
  await tap('Enter');
  expect(keys).toEqual(['Enter']);
});

it('Enter submits the form when keydown was not handled', async () => {
  await setup();
  const form = document.createElement('form');
  const input = document.createElement('input');
  form.append(input);
  document.body.append(form);
  form.requestSubmit = vi.fn();
  await focus(input);
  await tap('Enter');
  expect(form.requestSubmit).toHaveBeenCalled();
});

it('Done hides the keyboard and blurs the field', async () => {
  await setup();
  const input = field('<input type="text">');
  await focus(input);
  await tap('Done');
  expect(keyboard()).toBeNull();
  expect(document.activeElement).not.toBe(input);
});

it('switches to the symbols layer and back', async () => {
  await setup();
  const input = field('<input type="text">');
  await focus(input);
  await tap('Numbers and symbols');
  await tap('7');
  await tap('€');
  expect(input.value).toBe('7€');
  await tap('Letters');
  await tap('q');
  expect(input.value).toBe('7€q');
});

it('offers slash and dot for urls, at sign and dot for e-mail', async () => {
  await setup();
  await focus(field('<input type="url">'));
  expect(key('/')).toBeTruthy();
  await focus(field('<input type="email">'));
  expect(key('@')).toBeTruthy();
});

it('sets --osk-h while shown and removes it when hidden', async () => {
  await setup();
  const input = field('<input type="text">');
  await focus(input);
  expect(document.documentElement.style.getPropertyValue('--osk-h')).toMatch(/^\d+px$/);
  await tap('Done');
  expect(document.documentElement.style.getPropertyValue('--osk-h')).toBe('');
});
