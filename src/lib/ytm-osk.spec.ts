import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import source from '../../src-tauri/src/ytm/osk.js?raw';

type Hook = { root?: ShadowRoot; observer?: MutationObserver; navigate?: (url: string) => void };
type Win = Window & { __prismOskTest?: Hook; __prismOskBar?: boolean; __prismOsk?: boolean };

const w = window as unknown as Win;
let hook: Hook;
let navigated: string[];

/** Evaluates the script in the jsdom window. The hook opens the shadow root for the tests. */
function load(bar = true) {
  hook = { navigate: (url) => navigated.push(url) };
  w.__prismOskTest = hook;
  w.__prismOskBar = bar;
  (0, eval)(source);
}

function input(type?: string, attrs: Record<string, string> = {}): HTMLInputElement {
  const el = document.createElement('input');
  if (type) el.type = type;
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  document.body.appendChild(el);
  return el;
}

function key(label: string): HTMLElement {
  const found = [...hook.root!.querySelectorAll<HTMLElement>('.key')].find((k) => k.textContent === label || k.getAttribute('aria-label') === label);
  if (!found) throw new Error('no key ' + label);
  return found;
}

function tap(label: string) {
  // Resolved once: pressing shift or the layer key changes the label under the finger.
  const el = key(label);
  el.dispatchEvent(new Event('pointerdown', { bubbles: true, cancelable: true }));
  el.dispatchEvent(new Event('pointerup', { bubbles: true }));
}

const shown = () => hook.root!.querySelector('.osk')!.classList.contains('shown');

beforeEach(() => {
  navigated = [];
  document.documentElement.removeAttribute('style');
  document.body.innerHTML = '';
});

afterEach(async () => {
  // Lets the script's pending zero delay timers run while the document still exists.
  await new Promise((r) => setTimeout(r, 5));
  hook?.observer?.disconnect();
  document.querySelectorAll('prism-osk').forEach((e) => e.remove());
  delete w.__prismOsk;
  delete w.__prismOskTest;
  delete w.__prismOskBar;
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('on-screen keyboard script', () => {
  it('attaches one host element, also after a second evaluation', () => {
    load();
    (0, eval)(source);
    expect(document.querySelectorAll('prism-osk')).toHaveLength(1);
    expect(document.documentElement.lastElementChild?.tagName).toBe('PRISM-OSK');
  });

  it('shows the keyboard for text fields only', () => {
    load();
    input('checkbox').focus();
    expect(shown()).toBe(false);
    input('text', { readonly: '' }).focus();
    expect(shown()).toBe(false);
    input('text').focus();
    expect(shown()).toBe(true);
  });

  it('types at the caret and fires an input event', () => {
    load();
    const el = input('text');
    el.value = 'ac';
    el.focus();
    el.setSelectionRange(1, 1);
    const seen: string[] = [];
    el.addEventListener('input', (e) => seen.push((e as InputEvent).data ?? ''));
    tap('b');
    expect(el.value).toBe('abc');
    expect(seen).toEqual(['b']);
  });

  it('applies shift once and caps lock on a double tap', () => {
    load();
    const el = input('text');
    el.focus();
    tap('⇧');
    tap('A');
    tap('a');
    expect(el.value).toBe('Aa');
    tap('⇧');
    tap('⇧');
    tap('B');
    tap('C');
    expect(el.value).toBe('AaBC');
  });

  it('keeps ß as ß when shifted', () => {
    load();
    const el = input('text');
    el.focus();
    tap('⇧');
    tap('ß');
    expect(el.value).toBe('ß');
  });

  it('switches to the symbol layer and back', () => {
    load();
    const el = input('text');
    el.focus();
    tap('?123');
    tap('€');
    tap('ABC');
    tap('q');
    expect(el.value).toBe('€q');
  });

  it('offers at and dot beside space for e-mail fields', () => {
    load();
    input('email').focus();
    const row = [...hook.root!.querySelectorAll('.row')].pop()!;
    expect([...row.querySelectorAll('.key')].map((k) => k.textContent)).toContain('@');
  });

  it('deletes backward', () => {
    load();
    const el = input('text');
    el.value = 'abc';
    el.focus();
    el.setSelectionRange(3, 3);
    tap('⌫');
    expect(el.value).toBe('ab');
  });

  it('sends Enter as a keydown on the field', () => {
    load();
    const el = input('text');
    el.focus();
    const keys: string[] = [];
    el.addEventListener('keydown', (e) => keys.push(e.key));
    tap('↵');
    expect(keys).toEqual(['Enter']);
  });

  it('hides and blurs on Done', () => {
    load();
    const el = input('text');
    el.focus();
    tap('Done');
    expect(shown()).toBe(false);
    expect(document.activeElement).not.toBe(el);
  });

  it('hides when the field leaves the page', async () => {
    load();
    const el = input('text');
    el.focus();
    el.remove();
    await new Promise((r) => setTimeout(r, 0));
    expect(shown()).toBe(false);
  });

  it('adds the Cancel bar and top padding only when asked for', () => {
    load(true);
    expect(hook.root!.querySelector('.cancel')).not.toBeNull();
    expect(document.documentElement.style.paddingTop).toBe('44px');
    hook.root!.querySelector<HTMLElement>('.cancel')!.click();
    expect(navigated).toEqual(['https://prism.invalid/close-sign-in']);

    hook.observer?.disconnect();
    document.querySelector('prism-osk')!.remove();
    delete w.__prismOsk;
    document.documentElement.removeAttribute('style');
    load(false);
    expect(hook.root!.querySelector('.cancel')).toBeNull();
    expect(document.documentElement.style.paddingTop).toBe('');
  });

  it('makes no network or storage calls', () => {
    const fetchSpy = vi.fn();
    vi.stubGlobal('fetch', fetchSpy);
    const xhr = vi.spyOn(XMLHttpRequest.prototype, 'open');
    const get = vi.spyOn(Storage.prototype, 'getItem');
    const set = vi.spyOn(Storage.prototype, 'setItem');
    load();
    const el = input('text');
    el.focus();
    tap('a');
    tap('↵');
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(xhr).not.toHaveBeenCalled();
    expect(get).not.toHaveBeenCalled();
    expect(set).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });
});
