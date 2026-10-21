import { expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => undefined) }));

const { openOverlay, closeOverlay, anyOverlay, overlayOpen } = await import('./overlays.svelte');
const { overlays } = await import('./nav.svelte');

it('tracks ids as a set, so an id counts once', () => {
  expect(anyOverlay()).toBe(false);
  openOverlay('a');
  openOverlay('a');
  openOverlay('b');
  expect(anyOverlay()).toBe(true);
  expect(overlayOpen('a')).toBe(true);
  closeOverlay('a');
  expect(overlayOpen('a')).toBe(false);
  expect(anyOverlay()).toBe(true);
  closeOverlay('b');
  expect(anyOverlay()).toBe(false);
});

it('the About flag in nav is one overlay of the registry', () => {
  overlays.about = true;
  expect(overlayOpen('about')).toBe(true);
  expect(overlays.about).toBe(true);
  overlays.about = false;
  expect(anyOverlay()).toBe(false);
});
