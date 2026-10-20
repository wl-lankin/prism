import { flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';

const startResizeDragging = vi.fn(async (_direction: string) => {});
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ startResizeDragging }),
}));

const { default: ResizeBand } = await import('./ResizeBand.svelte');
const { windowState } = await import('../stores/window.svelte');

let app: ReturnType<typeof mount> | undefined;

afterEach(() => {
  if (app) unmount(app);
  app = undefined;
  windowState.maximized = false;
  startResizeDragging.mockClear();
});

function render(): HTMLElement {
  const target = document.createElement('div');
  app = mount(ResizeBand, { target, props: { gap: 108 } });
  flushSync();
  return target;
}

it('puts a bottom band beside the ring and two corner zones on a restored window', () => {
  const target = render();
  const zones = [...target.querySelectorAll<HTMLElement>('.resize')];
  expect(zones.map((zone) => zone.dataset.direction)).toEqual(['SouthWest', 'South', 'South', 'SouthEast']);
  // The ring's area stays free: the two South segments stop half the gap from the centre.
  const [, left, right] = zones;
  expect(left.getAttribute('style')).toContain('54px');
  expect(right.getAttribute('style')).toContain('54px');
});

it('starts a resize drag in the zone direction on a left mousedown', () => {
  const target = render();
  const zones = target.querySelectorAll<HTMLElement>('.resize');
  zones[1].dispatchEvent(new MouseEvent('mousedown', { button: 0, bubbles: true }));
  zones[3].dispatchEvent(new MouseEvent('mousedown', { button: 0, bubbles: true }));
  zones[0].dispatchEvent(new MouseEvent('mousedown', { button: 2, bubbles: true }));
  expect(startResizeDragging.mock.calls).toEqual([['South'], ['SouthEast']]);
});

it('has no band while the window is maximized', () => {
  windowState.maximized = true;
  const target = render();
  expect(target.querySelectorAll('.resize')).toHaveLength(0);

  windowState.maximized = false;
  flushSync();
  expect(target.querySelectorAll('.resize')).toHaveLength(4);
});
