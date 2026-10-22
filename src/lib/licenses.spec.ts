import { describe, expect, it } from 'vitest';
import pkg from '../../package.json';
import licenses from './licenses.json';

const key = (e: { name: string; version: string }) => `${e.name}\u0000${e.version}`;

describe('licenses.json', () => {
  const all = [...licenses.rust, ...licenses.npm, ...licenses.bundled];

  it('gives every entry a name, a version (except data) and a license', () => {
    expect(all.length).toBeGreaterThan(100);
    for (const e of all) {
      expect(e.name, JSON.stringify(e)).not.toBe('');
      expect(e.license, `license of ${e.name}`).not.toBe('');
    }
    for (const e of [...licenses.rust, ...licenses.npm]) expect(e.version, `version of ${e.name}`).not.toBe('');
  });

  it('is sorted, so regenerating it changes nothing', () => {
    for (const list of [licenses.rust, licenses.npm, licenses.bundled]) {
      const keys = list.map(key);
      expect(keys).toEqual([...keys].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)));
      expect(new Set(keys).size).toBe(keys.length);
    }
  });

  it('lists every production dependency of package.json', () => {
    const names = new Set(licenses.npm.map((e) => e.name));
    for (const name of Object.keys(pkg.dependencies)) expect(names.has(name), `${name} is missing, run npm run licenses`).toBe(true);
    expect(names.has('svelte')).toBe(true);
  });

  it('does not list dev-only tooling', () => {
    const names = new Set(licenses.npm.map((e) => e.name));
    for (const name of ['vitest', 'vite', 'jsdom', 'typescript']) expect(names.has(name)).toBe(false);
  });

  it('carries FFmpeg with its version and source, and the Navidrome logo', () => {
    const ffmpeg = licenses.bundled.find((e) => e.name === 'FFmpeg')!;
    expect(ffmpeg.license).toBe('LGPL-2.1-or-later');
    expect(ffmpeg.source).toBe(`https://ffmpeg.org/releases/ffmpeg-${ffmpeg.version}.tar.xz`);
    expect(licenses.bundled.find((e) => e.name === 'Navidrome logo')?.license).toBe('GPL-3.0');
  });
});
