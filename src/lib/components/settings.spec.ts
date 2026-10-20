import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

let stored: Record<string, unknown> = { language: 'en', volume: 0.8, outputDevice: null, gapless: true, replayGain: 'off' };

const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
  switch (cmd) {
    case 'library_folders':
      return [
        { path: 'C:/Demo/Music', online: true, trackCount: 1200 },
        { path: 'E:/Archive', online: false, trackCount: 1 },
      ];
    case 'audio_devices':
      return ['Speakers (Demo)', 'USB DAC'];
    case 'settings_set':
      stored = { ...stored, ...((args as { patch: object }).patch ?? {}) };
      return stored;
    default:
      return undefined;
  }
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(async () => null) }));
const getVersion = vi.fn(async () => '1.0.0');
vi.mock('@tauri-apps/api/app', () => ({ getVersion: () => getVersion() }));
const openUrl = vi.fn(async (_url: string) => {});
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: (url: string) => openUrl(url) }));

const { default: Select } = await import('./Select.svelte');
const { default: About } = await import('./About.svelte');
const { default: Settings } = await import('./Settings.svelte');
const { setLang } = await import('../i18n');
const { settings } = await import('../stores/settings.svelte');

let target: HTMLElement;
const mounted: ReturnType<typeof mount>[] = [];

beforeEach(() => {
  setLang('en');
  target = document.createElement('div');
  document.body.append(target);
  Element.prototype.scrollIntoView = vi.fn();
  invoke.mockClear();
  openUrl.mockClear();
});

afterEach(() => {
  for (const app of mounted.splice(0)) unmount(app);
  target.remove();
});

function key(element: Element, name: string, init: KeyboardEventInit = {}) {
  element.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true, ...init }));
  flushSync();
}

const options = [
  { value: '', label: 'System default' },
  { value: 'Speakers', label: 'Speakers' },
  { value: 'USB DAC', label: 'USB DAC' },
];

function mountSelect(value = '') {
  const onchange = vi.fn();
  mounted.push(mount(Select, { target, props: { value, options, label: 'Output device', onchange } }));
  flushSync();
  const button = target.querySelector<HTMLButtonElement>('[role="combobox"]')!;
  return { button, onchange };
}

it('opens with the keyboard on the selected option and picks with Enter', async () => {
  const { button, onchange } = mountSelect('Speakers');
  expect(button.getAttribute('aria-expanded')).toBe('false');
  key(button, 'ArrowDown');
  await tick();
  flushSync();
  expect(button.getAttribute('aria-expanded')).toBe('true');
  const list = target.querySelector('[role="listbox"]')!;
  expect(list.querySelectorAll('[role="option"]')).toHaveLength(3);
  expect(button.getAttribute('aria-activedescendant')).toBe(list.querySelectorAll('[role="option"]')[1].id);
  expect(list.querySelector('[aria-selected="true"]')?.textContent).toContain('Speakers');

  key(button, 'ArrowDown');
  expect(button.getAttribute('aria-activedescendant')).toBe(list.querySelectorAll('[role="option"]')[2].id);
  key(button, 'ArrowDown'); // stays on the last one
  expect(button.getAttribute('aria-activedescendant')).toBe(list.querySelectorAll('[role="option"]')[2].id);
  key(button, 'Home');
  expect(button.getAttribute('aria-activedescendant')).toBe(list.querySelectorAll('[role="option"]')[0].id);
  key(button, 'End');
  key(button, 'Enter');
  expect(onchange).toHaveBeenCalledWith('USB DAC');
  expect(button.getAttribute('aria-expanded')).toBe('false');
  expect(target.querySelector('[role="listbox"]')).toBeNull();
});

it('closes with Esc without choosing and jumps by typed letter', async () => {
  const { button, onchange } = mountSelect('');
  key(button, 'Enter');
  await tick();
  flushSync();
  const optionIds = [...target.querySelectorAll('[role="option"]')].map((o) => o.id);
  key(button, 'u');
  expect(button.getAttribute('aria-activedescendant')).toBe(optionIds[2]);
  const escape = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
  const outer = vi.fn();
  window.addEventListener('keydown', outer);
  button.dispatchEvent(escape);
  flushSync();
  window.removeEventListener('keydown', outer);
  expect(button.getAttribute('aria-expanded')).toBe('false');
  expect(outer).not.toHaveBeenCalled();
  expect(onchange).not.toHaveBeenCalled();
});

it('does not report the value that is already selected', async () => {
  const { button, onchange } = mountSelect('Speakers');
  key(button, ' ');
  await tick();
  key(button, 'Enter');
  expect(onchange).not.toHaveBeenCalled();
});

it('shows the About dialog with the version, links that open in the browser and Esc to close', async () => {
  const onclose = vi.fn();
  mounted.push(mount(About, { target, props: { onclose } }));
  flushSync();
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelector('.version')?.textContent).toBe('Version 1.0.0');
  });
  const dialog = target.querySelector('[role="dialog"]')!;
  expect(dialog.getAttribute('aria-modal')).toBe('true');
  expect(dialog.textContent).toContain('One player, every color of sound.');
  expect(dialog.textContent).toContain('Made by');
  expect(dialog.textContent).toContain('FFmpeg');
  expect(dialog.textContent).toContain('LGPL-2.1+');
  expect(document.activeElement?.classList.contains('close')).toBe(true);

  const author = [...dialog.querySelectorAll('a')].find((a) => a.textContent === 'Wolfgang Linz')!;
  author.click();
  expect(openUrl).toHaveBeenCalledWith('https://wolfgang-linz.de/');
  const github = dialog.querySelector<HTMLAnchorElement>('a.github')!;
  github.click();
  expect(openUrl).toHaveBeenLastCalledWith('https://github.com/wl-lankin/prism');

  // Focus stays inside: Tab from the last link wraps to the close button.
  github.focus();
  key(github, 'Tab');
  expect(document.activeElement?.classList.contains('close')).toBe(true);
  key(document.activeElement!, 'Tab', { shiftKey: true });
  expect(document.activeElement).toBe(github);

  key(dialog, 'Escape');
  expect(onclose).toHaveBeenCalledTimes(1);
  target.querySelector<HTMLElement>('.backdrop')!.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
  target.querySelector<HTMLElement>('.backdrop')!.click();
  expect(onclose).toHaveBeenCalledTimes(2);
});

it('speaks German in the About dialog', () => {
  setLang('de');
  mounted.push(mount(About, { target, props: { onclose: () => {} } }));
  flushSync();
  const text = target.querySelector('[role="dialog"]')!.textContent;
  expect(text).toContain('Ein Player, jede Farbe des Klangs.');
  expect(text).toContain('Entwickelt von');
  expect(text).toContain('Lizenzen');
});

it('lists the folders and saves playback settings at once', async () => {
  mounted.push(mount(Settings, { target }));
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelectorAll('.folder')).toHaveLength(2);
  });
  const rows = target.querySelectorAll('.folder');
  expect(rows[0].textContent).toContain('Online');
  expect(rows[0].textContent).toContain('1,200 tracks');
  expect(rows[1].textContent).toContain('Offline');

  const gapless = target.querySelector<HTMLButtonElement>('[role="switch"]')!;
  expect(gapless.getAttribute('aria-checked')).toBe('true');
  gapless.click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { gapless: false } }));
  flushSync();
  expect(gapless.getAttribute('aria-checked')).toBe('false');

  const album = target.querySelector<HTMLButtonElement>('[role="radio"][data-mode="album"]')!;
  album.click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { replayGain: 'album' } }));
  await vi.waitFor(() => expect(settings.replayGain).toBe('album'));
  expect(settings.gapless).toBe(false);

  // Removing asks once more before the folder goes.
  const remove = rows[0].querySelector<HTMLButtonElement>('.link')!;
  remove.click();
  flushSync();
  expect(invoke).not.toHaveBeenCalledWith('library_remove_folder', expect.anything());
  target.querySelector<HTMLButtonElement>('.link.danger')!.click();
  await vi.waitFor(() =>
    expect(invoke).toHaveBeenCalledWith('library_remove_folder', { path: 'C:/Demo/Music' }),
  );
});

it('offers the system default and every device, and moves to the chosen one', async () => {
  mounted.push(mount(Settings, { target }));
  const output = () => target.querySelector<HTMLButtonElement>('[aria-label="Output device"]')!;
  await vi.waitFor(() => {
    flushSync();
    expect(invoke).toHaveBeenCalledWith('audio_devices', undefined);
  });
  expect(output().textContent).toContain('System default');
  key(output(), 'ArrowDown');
  await tick();
  flushSync();
  const labels = [...target.querySelectorAll('[role="option"]')].map((o) => o.textContent?.trim());
  expect(labels).toEqual(['System default', 'Speakers (Demo)', 'USB DAC']);
  key(output(), 'End');
  key(output(), 'Enter');
  await vi.waitFor(() =>
    expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { outputDevice: 'USB DAC' } }),
  );
});
