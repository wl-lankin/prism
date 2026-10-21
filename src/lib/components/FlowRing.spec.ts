import { flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it } from 'vitest';
import shape from '../window-shape.json';

const { default: FlowRing } = await import('./FlowRing.svelte');

let app: ReturnType<typeof mount> | undefined;

afterEach(() => {
  if (app) unmount(app);
  app = undefined;
});

function render(props: { reach?: number; level?: number }) {
  const target = document.createElement('div');
  app = mount(FlowRing, { target, props: { mode: 'pause', progress: 0.3, size: 96, ...props } });
  flushSync();
  return target.querySelector<HTMLButtonElement>('.ring')!;
}

it('bounds its glow to the window shape it is given', () => {
  // The main window's region circle around the ring (`shell::shape::main_region`).
  const reach = shape.main.ring / 2 + shape.main.bezel + shape.main.glow;
  expect(reach).toBe(90);
  const ring = render({ reach, level: 1 });
  expect(ring.classList.contains('bounded')).toBe(true);
  expect(ring.style.getPropertyValue('--reach')).toBe('90px');
  // At full level the glow is at its largest; it stays inside the bound.
  expect(ring.style.getPropertyValue('--glow-s')).toBe('1.120');
});

it('leaves the glow unbounded without a shape (maximized window, mini player)', () => {
  const ring = render({});
  expect(ring.classList.contains('bounded')).toBe(false);
  expect(ring.style.getPropertyValue('--reach')).toBe('');
});
