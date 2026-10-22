import { flushSync, mount, unmount } from 'svelte';
import { expect, it, vi } from 'vitest';
import { SPECTRUM } from '../palette';
import { ring } from '../stores/ring.svelte';
import Slider from './Slider.svelte';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

// jsdom has no pointer capture or PointerEvent.
HTMLElement.prototype.setPointerCapture = () => {};
const pointer = (type: string, clientX = 0) =>
  Object.assign(new MouseEvent(type, { bubbles: true, clientX, button: 0 }), { pointerId: 1 });

function setup() {
  const oninput = vi.fn();
  const oncommit = vi.fn();
  const oncancel = vi.fn();
  const target = document.createElement('div');
  const app = mount(Slider, { target, props: { value: 0.2, label: 'Seek', oninput, oncommit, oncancel } });
  flushSync();
  const el = target.querySelector<HTMLElement>('[role="slider"]')!;
  return { app, el, oninput, oncommit, oncancel };
}

it('reports a cancelled drag when the pointer capture is lost', () => {
  const { app, el, oncommit, oncancel } = setup();
  el.dispatchEvent(pointer('pointerdown'));
  el.dispatchEvent(pointer('lostpointercapture'));
  expect(oncancel).toHaveBeenCalledTimes(1);
  expect(oncommit).not.toHaveBeenCalled();
  unmount(app);
});

it('reports a cancelled drag on pointercancel', () => {
  const { app, el, oncancel } = setup();
  el.dispatchEvent(pointer('pointerdown'));
  el.dispatchEvent(pointer('pointercancel'));
  expect(oncancel).toHaveBeenCalledTimes(1);
  unmount(app);
});

it('commits on release without a cancel', () => {
  const { app, el, oncommit, oncancel } = setup();
  el.dispatchEvent(pointer('pointerdown'));
  el.dispatchEvent(pointer('pointerup'));
  el.dispatchEvent(pointer('lostpointercapture'));
  expect(oncommit).toHaveBeenCalledTimes(1);
  expect(oncancel).not.toHaveBeenCalled();
  unmount(app);
});

function fillStyle(spectrum: boolean) {
  const target = document.createElement('div');
  const app = mount(Slider, { target, props: { value: 0.5, label: 'Seek', spectrum } });
  flushSync();
  const style = target.querySelector<HTMLElement>('.fill')!.getAttribute('style') ?? '';
  return { app, style };
}

const seekColors = (style: string) => [1, 2, 3, 4].map((i) => style.match(new RegExp(`--seek-${i}: ?([^;]+)`))?.[1]);

it('paints the seek fill with the ring colors, in order', () => {
  const cover: [string, string, string, string] = ['#3df57a', '#3df5c4', '#3dc4f5', '#7af53d'];
  ring.colors = cover;
  const { app, style } = fillStyle(true);
  expect(seekColors(style)).toEqual(cover);
  unmount(app);
});

it('falls back to the spectrum without a cover', () => {
  ring.colors = SPECTRUM;
  const { app, style } = fillStyle(true);
  expect(seekColors(style)).toEqual([...SPECTRUM]);
  unmount(app);
});

it('sets no cover colors on a plain slider', () => {
  const { app, style } = fillStyle(false);
  expect(style).not.toContain('--seek-');
  unmount(app);
});
