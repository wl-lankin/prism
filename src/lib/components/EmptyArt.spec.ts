import { flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it } from 'vitest';
import EmptyArt, { type EmptyKind } from './EmptyArt.svelte';

let app: ReturnType<typeof mount> | undefined;

afterEach(() => {
  if (app) unmount(app);
  app = undefined;
});

function render(props: { kind: EmptyKind; accent?: string }) {
  const target = document.createElement('div');
  app = mount(EmptyArt, { target, props });
  flushSync();
  return target.querySelector<SVGSVGElement>('svg.empty-art')!;
}

const KINDS: EmptyKind[] = ['songs', 'albums', 'artists', 'stations', 'playlists', 'library', 'search'];

it.each(KINDS)('renders the %s art as a decorative stack with a glyph', (kind) => {
  const svg = render({ kind });
  expect(svg.getAttribute('aria-hidden')).toBe('true');
  expect(svg.dataset.kind).toBe(kind);
  expect(svg.getAttribute('viewBox')).toBe('0 0 136 108');
  // Three slabs, each with its inner edge.
  expect(svg.querySelectorAll('rect[rx="13.5"]')).toHaveLength(3);
  expect(svg.querySelector('.glyph')!.children.length).toBeGreaterThan(0);
  if (app) unmount(app);
  app = undefined;
});

it('leans the light line and the glow toward a given accent', () => {
  const svg = render({ kind: 'search', accent: 'var(--src-radio)' });
  const styles = (suffix: string) =>
    [...svg.querySelectorAll('stop')]
      .filter((stop) => stop.parentElement!.id.endsWith(suffix))
      .map((stop) => stop.getAttribute('style') ?? '');
  expect(styles('-line').some((s) => s.includes('var(--src-radio)'))).toBe(true);
  expect(styles('-glow')[0]).toContain('var(--src-radio)');
});

it('gives every instance its own gradient ids', () => {
  const first = document.createElement('div');
  const second = document.createElement('div');
  const a = mount(EmptyArt, { target: first, props: { kind: 'songs' } });
  const b = mount(EmptyArt, { target: second, props: { kind: 'songs' } });
  flushSync();
  const id = (root: HTMLElement) => root.querySelector('radialGradient')!.id;
  expect(id(first)).not.toBe(id(second));
  unmount(a);
  unmount(b);
});
