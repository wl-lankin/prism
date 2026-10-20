import { describe, expect, it } from 'vitest';
import { anchorOffset, gridLayout, visibleRange } from './virtual';

describe('visibleRange', () => {
  it('starts at the top with the overscan after the viewport', () => {
    expect(visibleRange(0, 440, 44, 30000, 5)).toEqual({ start: 0, end: 15 });
  });

  it('adds the overscan on both sides in the middle of the list', () => {
    expect(visibleRange(44 * 1000, 440, 44, 30000, 5)).toEqual({ start: 995, end: 1015 });
  });

  it('clamps the end at total', () => {
    expect(visibleRange(44 * 29995, 440, 44, 30000, 5)).toEqual({ start: 29990, end: 30000 });
    expect(visibleRange(0, 440, 44, 3, 5)).toEqual({ start: 0, end: 3 });
  });

  it('handles an empty list and scroll positions past the end', () => {
    expect(visibleRange(0, 440, 44, 0, 5)).toEqual({ start: 0, end: 0 });
    expect(visibleRange(44 * 50, 440, 44, 10, 5)).toEqual({ start: 10, end: 10 });
  });

  it('treats a negative scroll (overscroll bounce) as the top', () => {
    expect(visibleRange(-120, 440, 44, 100, 2)).toEqual({ start: 0, end: 12 });
  });

  it('covers a row that is only partly visible', () => {
    expect(visibleRange(22, 44, 44, 100, 0)).toEqual({ start: 0, end: 2 });
  });
});

describe('gridLayout', () => {
  it('fits as many columns of at least the minimum width as the width allows', () => {
    // 4 columns of 180 plus 3 gaps of 24 = 792.
    expect(gridLayout(792, 180, 24)).toEqual({ columns: 4, cardWidth: 180 });
    expect(gridLayout(800, 180, 24)).toEqual({ columns: 4, cardWidth: 182 });
  });

  it('stretches the cards to fill the row', () => {
    const { columns, cardWidth } = gridLayout(1000, 180, 24);
    expect(columns).toBe(5);
    expect(columns * cardWidth + (columns - 1) * 24).toBeCloseTo(1000, 5);
  });

  it('always keeps one column', () => {
    expect(gridLayout(100, 180, 24)).toEqual({ columns: 1, cardWidth: 100 });
    expect(gridLayout(0, 180, 24).columns).toBe(1);
  });
});

describe('anchorOffset', () => {
  it('keeps the first visible card at the top when the column count changes', () => {
    // Row 10 of a 4-column grid starts with card 40; with 5 columns that is row 8.
    expect(anchorOffset(10 * 250, 250, 4, 200, 5)).toBe(8 * 200);
    // Half a row scrolled past stays half a row.
    expect(anchorOffset(10.5 * 250, 250, 4, 200, 5)).toBe(8.5 * 200);
    // Card 40 in a 3-column grid is in row 13.
    expect(anchorOffset(10 * 250, 250, 4, 300, 3)).toBe(13 * 300);
  });

  it('stays at the top', () => {
    expect(anchorOffset(0, 250, 4, 200, 5)).toBe(0);
  });
});
