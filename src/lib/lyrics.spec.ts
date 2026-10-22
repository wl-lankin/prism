import { expect, it } from 'vitest';
import { activeLine, interpolate, LEAD_MS } from './lyrics';

const lines = [0, 1000, 2000, 5000].map((startMs, i) => ({ startMs, text: `Line ${i}` }));

it('picks the last line that started, with the lead', () => {
  expect(activeLine(lines, 0)).toBe(0);
  expect(activeLine(lines, 999 - LEAD_MS)).toBe(0);
  expect(activeLine(lines, 1000 - LEAD_MS)).toBe(1);
  expect(activeLine(lines, 4849)).toBe(2);
  expect(activeLine(lines, 4850)).toBe(3);
  expect(activeLine(lines, 99_000)).toBe(3);
});

it('has no active line before the first one or without lines', () => {
  const late = [{ startMs: 5000, text: 'Late' }];
  expect(activeLine(late, 0)).toBe(-1);
  expect(activeLine(late, 4850)).toBe(0);
  expect(activeLine([], 1000)).toBe(-1);
});

it('moves the position on with the clock only while playing', () => {
  const base = { positionMs: 10_000, at: 500 };
  expect(interpolate(base, 700, true, 200_000)).toBe(10_200);
  expect(interpolate(base, 700, false, 200_000)).toBe(10_000);
  expect(interpolate(base, 700, true, 10_100)).toBe(10_100);
  expect(interpolate(base, 100, true, 0)).toBe(10_000);
});
