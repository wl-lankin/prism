import { beforeEach, expect, it } from 'vitest';
import { back, go, nav } from './nav.svelte';

/** More than the trail ever holds (`TRAIL_MAX` is 30). */
const DRAIN = 64;

beforeEach(() => {
  // Empty the trail, so every test starts with no place to go back to.
  for (let i = 0; i < DRAIN; i++) back();
  nav.view = 'albums';
  nav.param = undefined;
  nav.source = undefined;
});

it('treats another source as another place, and back returns to all albums', () => {
  go('albums', undefined, 'navidrome');
  expect(nav).toMatchObject({ view: 'albums', source: 'navidrome' });
  back();
  expect(nav.view).toBe('albums');
  expect(nav.source).toBeUndefined();
});

it('ignores a go to the place it already is', () => {
  go('songs');
  go('albums', undefined, 'local');
  go('albums', undefined, 'local');
  back();
  expect(nav).toMatchObject({ view: 'songs', source: undefined });
  back();
  expect(nav).toMatchObject({ view: 'albums', source: undefined });
});

it('starts each test with an empty trail', () => {
  // With nothing to return to, back goes to the albums.
  nav.view = 'songs';
  back();
  expect(nav).toMatchObject({ view: 'albums', source: undefined });
});

it('keeps the source of a place in the trail', () => {
  go('artists', undefined, 'navidrome');
  go('artist', 'Air');
  expect(nav.source).toBeUndefined();
  back();
  expect(nav).toMatchObject({ view: 'artists', source: 'navidrome' });
});
