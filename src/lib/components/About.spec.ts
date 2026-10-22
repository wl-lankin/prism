import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/app', () => ({ getVersion: async () => '1.0.0' }));
const openUrl = vi.fn(async (_url: string) => {});
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: (url: string) => openUrl(url) }));

const { default: About } = await import('./About.svelte');
const { default: licenses } = await import('../licenses.json');
const { setLang } = await import('../i18n');

let target: HTMLElement;
const mounted: ReturnType<typeof mount>[] = [];

beforeEach(() => {
  setLang('en');
  target = document.createElement('div');
  document.body.append(target);
  openUrl.mockClear();
});

afterEach(() => {
  mounted.splice(0).forEach((app) => unmount(app));
  target.remove();
});

async function openList() {
  mounted.push(mount(About, { target, props: { onclose: () => {} } }));
  flushSync();
  target.querySelector<HTMLButtonElement>('.open')!.click();
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelector('h4')).not.toBeNull();
  });
}

it('opens the generated license list in three groups, with the bundled programs first', async () => {
  await openList();
  const groups = [...target.querySelectorAll('h4')].map((h) => h.dataset.group);
  expect(groups).toEqual(['bundled', 'npm', 'rust']);
  const text = target.querySelector('.licenses')!.textContent!;
  expect(text).toContain('FFmpeg');
  expect(text).toContain('LGPL-2.1-or-later');
  expect(text).toContain('Navidrome logo');
  expect(text).toContain('GPL-3.0');
  expect(text).toContain('Radio Browser');
  expect(text).toContain('GeoJS');
  expect(text).toContain('symphonia');
  expect(target.querySelectorAll('dl > div').length).toBe(licenses.rust.length + licenses.npm.length + licenses.bundled.length);
  // The focus lands in the search field.
  expect(document.activeElement).toBe(target.querySelector('.search'));
});

it('filters by name or license, and says so when nothing matches', async () => {
  await openList();
  const search = target.querySelector<HTMLInputElement>('.search')!;
  search.value = 'symphonia-core';
  search.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
  expect([...target.querySelectorAll('h4')].map((h) => h.dataset.group)).toEqual(['rust']);
  expect(target.querySelectorAll('dl > div')).toHaveLength(1);

  search.value = 'lgpl-2.1';
  search.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
  expect(target.querySelector('dl')!.textContent).toContain('FFmpeg');

  search.value = 'no such crate';
  search.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
  expect(target.querySelector('.none')?.textContent).toBe('Nothing matches.');
});

it('opens a package link in the browser and goes back to the credits', async () => {
  await openList();
  const ffmpeg = [...target.querySelectorAll<HTMLAnchorElement>('dt a')].find((a) => a.textContent === 'FFmpeg')!;
  ffmpeg.click();
  expect(openUrl).toHaveBeenCalledWith('https://github.com/wl-lankin/prism/blob/main/docs/ffmpeg.md');
  const source = [...target.querySelectorAll<HTMLAnchorElement>('a.note')].find((a) => a.textContent === 'Source')!;
  source.click();
  expect(openUrl).toHaveBeenLastCalledWith(expect.stringMatching(/^https:\/\/ffmpeg\.org\/releases\/ffmpeg-[\d.]+\.tar\.xz$/));

  target.querySelector<HTMLButtonElement>('.back')!.click();
  await tick();
  flushSync();
  expect(target.querySelector('.search')).toBeNull();
  expect(target.querySelector('.open')).not.toBeNull();
});

it('translates the list in German', async () => {
  setLang('de');
  await openList();
  const text = target.querySelector('.licenses')!.textContent!;
  expect(text).toContain('Mitgelieferte Programme und Daten');
  expect(text).toContain('JavaScript-Pakete');
  expect(text).toContain('Läuft als eigenes Programm');
});
