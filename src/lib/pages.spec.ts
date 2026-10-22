import { describe, expect, it } from 'vitest';
import { evictPages, pagesFor } from './pages';

describe('pagesFor', () => {
  it('lists every page a row range touches', () => {
    expect(pagesFor(0, 15, 200)).toEqual([0]);
    expect(pagesFor(195, 215, 200)).toEqual([0, 1]);
    expect(pagesFor(400, 400, 200)).toEqual([]);
  });
});

describe('evictPages', () => {
  it('keeps everything while the cache is within the cap', () => {
    expect(evictPages([0, 1, 2], [1], 20)).toEqual([]);
  });

  it('evicts the pages farthest from the visible ones first', () => {
    const cached = Array.from({ length: 25 }, (_, i) => i);
    // Visible: page 12. Farthest: 0, 24, 1, 23, 2 (ties go to the lower page first).
    expect(evictPages(cached, [12], 20).sort((a, b) => a - b)).toEqual([0, 1, 2, 23, 24]);
  });

  it('never evicts a visible page', () => {
    expect(evictPages([5, 6, 50], [5, 6], 1).sort((a, b) => a - b)).toEqual([50]);
  });

  it('measures the distance to the nearest visible page', () => {
    // Visible 10..11: page 3 is 7 away, page 15 is 4 away, page 30 is 19 away.
    expect(evictPages([3, 10, 11, 15, 30], [10, 11], 3).sort((a, b) => a - b)).toEqual([3, 30]);
  });
});
