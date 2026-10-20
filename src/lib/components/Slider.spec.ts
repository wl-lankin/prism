import { flushSync, mount, unmount } from 'svelte';
import { expect, it, vi } from 'vitest';
import Slider from './Slider.svelte';

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
