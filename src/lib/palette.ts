import { coverUrl } from './ipc';

/** The ring's four gradient stops, `#rrggbb`. */
export type RingColors = [string, string, string, string];

/** Prism's own colors: the ring without a cover, or with one that has no color to give. */
export const SPECTRUM: RingColors = ['#ff4d6d', '#ffc94d', '#4dffb2', '#4da6ff'];

const BINS = 12;
const BIN_DEG = 360 / BINS;
const MIN_SATURATION = 0.2;
const MIN_LIGHTNESS = 0.12;
const MAX_LIGHTNESS = 0.92;
/** Fewer chromatic pixels than this share: the cover is treated as gray. */
const MIN_CHROMATIC = 0.03;
/** A bin with less than this share of the color weight is noise, not a color of the cover. */
const MIN_BIN_SHARE = 0.02;
/** Every ring color has the same saturation and lightness, so any cover sits right on the
 *  dark card and the four stops blend like the spectrum does. */
const OUT_SATURATION = 0.9;
const OUT_LIGHTNESS = 0.64;
/** Hue turns used for stops the cover cannot fill, in order. */
const FILL_TURNS = [40, -40, 80];

function hsl(r: number, g: number, b: number): [h: number, s: number, l: number] {
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const d = max - min;
  if (d === 0) return [0, 0, l];
  const s = d / (1 - Math.abs(2 * l - 1));
  const h = max === r ? ((g - b) / d + 6) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return [h * 60, s, l];
}

function hex(h: number, s: number, l: number): string {
  const a = s * Math.min(l, 1 - l);
  const channel = (n: number) => {
    const k = (n + h / 30) % 12;
    const v = l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1));
    return Math.round(v * 255)
      .toString(16)
      .padStart(2, '0');
  };
  return `#${channel(0)}${channel(8)}${channel(4)}`;
}

const turn = (h: number, by: number) => (((h + by) % 360) + 360) % 360;

/**
 * The ring colors of an image (RGBA pixels, as `getImageData` gives them): its strongest
 * hues, from 12 hue bins weighted by saturation. Pixels that are nearly gray, nearly black or
 * nearly white do not count. Up to four bins give a color each (a bin's own mean hue);
 * missing ones are the strongest hue turned by 40 degrees either way. The colors run by hue
 * from the strongest. `null` when under 3 % of the pixels have color.
 */
export function paletteFromPixels(data: Uint8ClampedArray): RingColors | null {
  const weight = new Float64Array(BINS);
  const sin = new Float64Array(BINS);
  const cos = new Float64Array(BINS);
  let pixels = 0;
  let chromatic = 0;
  for (let i = 0; i + 3 < data.length; i += 4) {
    if (data[i + 3] < 128) continue;
    pixels++;
    const [h, s, l] = hsl(data[i] / 255, data[i + 1] / 255, data[i + 2] / 255);
    if (s < MIN_SATURATION || l < MIN_LIGHTNESS || l > MAX_LIGHTNESS) continue;
    chromatic++;
    // Bins are centered on 0, 30, 60 ... degrees, so reds near 0 and 360 share one.
    const bin = Math.floor(turn(h, BIN_DEG / 2) / BIN_DEG) % BINS;
    const rad = (h * Math.PI) / 180;
    weight[bin] += s;
    sin[bin] += Math.sin(rad) * s;
    cos[bin] += Math.cos(rad) * s;
  }
  if (pixels === 0 || chromatic / pixels < MIN_CHROMATIC) return null;

  const total = weight.reduce((sum, w) => sum + w, 0);
  const strongest = [...weight.keys()]
    .filter((bin) => weight[bin] >= total * MIN_BIN_SHARE)
    .sort((a, b) => weight[b] - weight[a] || a - b)
    .slice(0, 4);
  const hues = strongest.map((bin) => turn((Math.atan2(sin[bin], cos[bin]) * 180) / Math.PI, 0));
  const first = hues[0];
  for (const by of FILL_TURNS) {
    if (hues.length >= 4) break;
    hues.push(turn(first, by));
  }
  const ordered = [first, ...hues.slice(1).sort((a, b) => turn(a, -first) - turn(b, -first))];
  return ordered.map((h) => hex(h, OUT_SATURATION, OUT_LIGHTNESS)) as RingColors;
}

/** Colors per cover id; a cover's colors never change. */
const cache = new Map<string, Promise<RingColors>>();

function pixelsOf(src: string, size: number): Promise<Uint8ClampedArray | null> {
  return new Promise((resolve) => {
    const img = new Image();
    // The cover protocol allows any origin, so the canvas stays readable.
    img.crossOrigin = 'anonymous';
    img.decoding = 'async';
    img.onload = () => {
      try {
        const canvas = document.createElement('canvas');
        canvas.width = size;
        canvas.height = size;
        const context = canvas.getContext('2d', { willReadFrequently: true });
        if (!context) return resolve(null);
        context.drawImage(img, 0, 0, size, size);
        resolve(context.getImageData(0, 0, size, size).data);
      } catch {
        resolve(null);
      }
    };
    img.onerror = () => resolve(null);
    img.src = src;
  });
}

/** The ring colors for a cover: drawn from its 96 px image, the spectrum without one. A cover
 *  that cannot be loaded gets the spectrum too, and is tried again next time. */
export function ringColorsFor(coverId: string | null): Promise<RingColors> {
  if (!coverId) return Promise.resolve(SPECTRUM);
  const known = cache.get(coverId);
  if (known) return known;
  const src = coverUrl(coverId, 96);
  if (!src) return Promise.resolve(SPECTRUM);
  const colors = pixelsOf(src, 96).then((data) => {
    if (!data) {
      cache.delete(coverId);
      return SPECTRUM;
    }
    return paletteFromPixels(data) ?? SPECTRUM;
  });
  cache.set(coverId, colors);
  return colors;
}
