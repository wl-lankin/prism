import { flushSync, mount, tick, unmount } from 'svelte';
import { SvelteMap } from 'svelte/reactivity';
import { afterEach, expect, it } from 'vitest';
import Segmented from './Segmented.svelte';

// jsdom has no layout: give each option a position and width by its place (widths differ).
const WIDTHS: Record<string, number> = { off: 50, flow: 80, always: 120 };
const ORDER = ['off', 'flow', 'always'];
const offsetOf = (el: HTMLElement) => {
  const key = el.dataset.crossfade!;
  return ORDER.slice(0, ORDER.indexOf(key)).reduce((sum, k) => sum + WIDTHS[k] + 2, 3);
};
Object.defineProperty(HTMLElement.prototype, 'offsetLeft', { configurable: true, get() { return this.dataset?.crossfade ? offsetOf(this) : 0; } });
Object.defineProperty(HTMLElement.prototype, 'offsetWidth', { configurable: true, get() { return this.dataset?.crossfade ? WIDTHS[this.dataset.crossfade] : 0; } });

const options = ORDER.map((value) => ({ value, label: value }));
const apps: object[] = [];
afterEach(() => apps.splice(0).forEach((app) => unmount(app)));

async function setup() {
  const target = document.createElement('div');
  document.body.append(target);
  const state = new SvelteMap<string, string>([['value', 'off']]);
  const props = {
    get value() {
      return state.get('value')!;
    },
    set value(v: string) {
      state.set('value', v);
    },
  };
  apps.push(mount(Segmented, { target, props: { options, labelledby: 'x', attr: 'data-crossfade', onpick: (v: string) => (props.value = v), get value() { return props.value; } } }));
  flushSync();
  await tick();
  flushSync();
  const marker = target.querySelector<HTMLElement>('.marker')!;
  return { target, props, marker };
}

it('moves one marker to the active option, with its offset and width', async () => {
  const { target, props, marker } = await setup();
  expect(marker.style.transform).toBe('translateX(3px)');
  expect(marker.style.width).toBe('50px');

  props.value = 'always';
  flushSync();
  await tick();
  flushSync();
  const active = target.querySelector<HTMLElement>('[aria-checked="true"]')!;
  expect(active.dataset.crossfade).toBe('always');
  expect(marker.style.transform).toBe(`translateX(${active.offsetLeft}px)`);
  expect(marker.style.transform).toBe('translateX(137px)');
  expect(marker.style.width).toBe(`${active.offsetWidth}px`);
  expect(target.querySelectorAll('.marker')).toHaveLength(1);
});

it('picks by click and arrow key', async () => {
  const { target, props } = await setup();
  target.querySelector<HTMLButtonElement>('[data-crossfade="flow"]')!.click();
  expect(props.value).toBe('flow');
  target.querySelector<HTMLButtonElement>('[data-crossfade="flow"]')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
  expect(props.value).toBe('always');
});
