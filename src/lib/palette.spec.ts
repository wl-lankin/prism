import { describe, expect, it } from 'vitest';
import { paletteFromPixels, SPECTRUM } from './palette';

/** RGBA pixels: `count` pixels of each color in turn. */
function image(...runs: [rgb: [number, number, number], count: number][]): Uint8ClampedArray {
  const total = runs.reduce((sum, [, count]) => sum + count, 0);
  const data = new Uint8ClampedArray(total * 4);
  let i = 0;
  for (const [[r, g, b], count] of runs) {
    for (let n = 0; n < count; n++) {
      data.set([r, g, b, 255], i);
      i += 4;
    }
  }
  return data;
}

function hue(hex: string): number {
  const n = parseInt(hex.slice(1), 16);
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => v / 255);
  const max = Math.max(r, g, b);
  const d = max - Math.min(r, g, b);
  if (d === 0) return 0;
  const h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return (h * 60 + 360) % 360;
}

function near(a: number, b: number, within: number): boolean {
  const d = Math.abs(a - b) % 360;
  return Math.min(d, 360 - d) <= within;
}

describe('paletteFromPixels', () => {
  it('gives four colors with a red and a blue for a half red, half blue cover', () => {
    const colors = paletteFromPixels(image([[220, 30, 50], 48 * 96], [[40, 70, 230], 48 * 96]));
    expect(colors).not.toBeNull();
    expect(colors).toHaveLength(4);
    const hues = colors!.map(hue);
    expect(hues.some((h) => near(h, 350, 20))).toBe(true);
    expect(hues.some((h) => near(h, 230, 20))).toBe(true);
  });

  it('outputs #rrggbb strings', () => {
    const colors = paletteFromPixels(image([[255, 0, 0], 500], [[0, 0, 255], 500], [[0, 200, 0], 500]));
    for (const color of colors!) expect(color).toMatch(/^#[0-9a-f]{6}$/);
  });

  it('starts at the strongest hue and orders the rest by hue', () => {
    const colors = paletteFromPixels(
      image([[40, 70, 230], 600], [[220, 30, 50], 300], [[30, 200, 90], 200], [[230, 200, 40], 100]),
    )!;
    const hues = colors.map(hue);
    expect(near(hues[0], 230, 20)).toBe(true);
    const turned = hues.map((h) => (h - hues[0] + 360) % 360);
    expect([...turned].sort((a, b) => a - b)).toEqual(turned);
  });

  it('gives null for a grayscale cover', () => {
    const grays = Array.from({ length: 256 }, (_, v) => [[v, v, v], 20] as [[number, number, number], number]);
    expect(paletteFromPixels(image(...grays))).toBeNull();
  });

  it('gives null when under 3 % of the pixels have color', () => {
    expect(paletteFromPixels(image([[128, 128, 128], 980], [[255, 0, 0], 20]))).toBeNull();
    expect(paletteFromPixels(image([[128, 128, 128], 960], [[255, 0, 0], 40]))).not.toBeNull();
  });

  it('keeps the spectrum as four #rrggbb colors', () => {
    expect(SPECTRUM).toHaveLength(4);
  });
});
