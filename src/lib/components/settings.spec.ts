import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { NavidromeStatus } from '../types';

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
const { settings, updateSettings, DEFAULT_SETTINGS } = await import('../stores/settings.svelte');
const { placeList } = await import('./Select.svelte');
const { navidrome } = await import('../stores/navidrome.svelte');

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

it('marks the chosen option and opens on Home and End', async () => {
  const { button } = mountSelect('Speakers');
  key(button, 'End');
  await tick();
  flushSync();
  expect(button.getAttribute('aria-expanded')).toBe('true');
  const all = [...target.querySelectorAll('[role="option"]')];
  expect(all.map((o) => o.getAttribute('aria-selected'))).toEqual(['false', 'true', 'false']);
  expect(button.getAttribute('aria-activedescendant')).toBe(all[2].id);
  key(button, 'Escape');
  key(button, 'Home');
  await tick();
  flushSync();
  expect(button.getAttribute('aria-expanded')).toBe('true');
  expect(button.getAttribute('aria-activedescendant')).toBe(target.querySelectorAll('[role="option"]')[0].id);
});

it('matches a prefix typed within half a second', async () => {
  const now = vi.spyOn(Date, 'now').mockReturnValue(10_000);
  const genres = ['System default', 'Rap', 'Rock', 'Roots', 'Pop'].map((label) => ({ value: label, label }));
  mounted.push(mount(Select, { target, props: { value: 'System default', options: genres, label: 'Genre' } }));
  flushSync();
  const button = target.querySelector<HTMLButtonElement>('[role="combobox"]')!;
  key(button, 'Enter');
  await tick();
  flushSync();
  const ids = [...target.querySelectorAll('[role="option"]')].map((o) => o.id);
  key(button, 'r');
  expect(button.getAttribute('aria-activedescendant')).toBe(ids[1]);
  now.mockReturnValue(10_200);
  key(button, 'o');
  expect(button.getAttribute('aria-activedescendant')).toBe(ids[2]);
  now.mockReturnValue(10_400);
  key(button, 'o');
  expect(button.getAttribute('aria-activedescendant')).toBe(ids[3]);
  // A pause starts over: "r" again moves on to the next match.
  now.mockReturnValue(11_000);
  key(button, 'r');
  expect(button.getAttribute('aria-activedescendant')).toBe(ids[1]);
  now.mockRestore();
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
  // Shortcuts behind the dialog (Ctrl+K search) do not see keys while it is open.
  const behind = vi.fn();
  window.addEventListener('keydown', behind);
  key(document.body, 'k', { ctrlKey: true });
  window.removeEventListener('keydown', behind);
  expect(behind).not.toHaveBeenCalled();
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

it('saves the crossfade, mini player, cover and system settings', async () => {
  mounted.push(mount(Settings, { target }));
  flushSync();
  const always = target.querySelector<HTMLButtonElement>('[role="radio"][data-crossfade="always"]')!;
  always.click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { crossfade: 'always' } }));

  const length = target.querySelector<HTMLElement>('[role="slider"][aria-label="Crossfade length"]')!;
  expect(length.getAttribute('aria-valuetext')).toBe('4 s');
  key(length, 'ArrowRight');
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { crossfadeMs: 5000 } }));

  const toggle = (label: string) => target.querySelector<HTMLButtonElement>(`[role="switch"][aria-labelledby="${label}"]`)!;
  toggle('label-on-top').click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { miniOnTop: true } }));
  toggle('label-covers').click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { coverLookup: false } }));
  expect(target.textContent).toContain('MusicBrainz and the Cover Art Archive');
  toggle('label-autostart').click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { startWithSystem: true } }));
});

it('shows why a shortcut was refused and keeps the old one', async () => {
  await updateSettings({ miniShortcut: 'CommandOrControl+Alt+P' });
  mounted.push(mount(Settings, { target }));
  flushSync();
  invoke.mockImplementationOnce(async () => {
    throw 'shortcut taken: CommandOrControl+Alt+T (HotKey already registered)';
  });
  const recorder = target.querySelector<HTMLButtonElement>('.shortcut')!;
  recorder.click();
  flushSync();
  key(recorder, 't', { code: 'KeyT', ctrlKey: true, altKey: true });
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelector('[role="alert"]')?.textContent).toBe(
      'Ctrl+Alt+T is already used by another app. Pick another combination.',
    );
  });
  expect(settings.miniShortcut).toBe('CommandOrControl+Alt+P');
  expect(recorder.textContent).toContain('Ctrl+Alt+P');
  const error = target.querySelector('[role="alert"]')!;
  expect(recorder.getAttribute('aria-describedby')).toBe(error.id);
});

it('flips start with system back when the system refuses it', async () => {
  await updateSettings({ startWithSystem: false });
  mounted.push(mount(Settings, { target }));
  flushSync();
  invoke.mockImplementationOnce(async () => {
    throw 'turning on the start with the system failed: access denied';
  });
  const toggle = target.querySelector<HTMLButtonElement>('[role="switch"][aria-labelledby="label-autostart"]')!;
  toggle.click();
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelector('[role="alert"]')?.textContent).toContain('The start with the system could not be changed');
  });
  expect(settings.startWithSystem).toBe(false);
  expect(toggle.getAttribute('aria-checked')).toBe('false');
});

it('places the list inside the clipping scroller, flipping up when below is short', () => {
  const clip = { top: 60, bottom: 700 };
  expect(placeList({ top: 100, bottom: 136 }, clip, 212)).toEqual({ upward: false, room: 300 });
  // Near the scroller's bottom edge (the transport strip below it): up, with all the room.
  expect(placeList({ top: 600, bottom: 636 }, clip, 212)).toEqual({ upward: true, room: 300 });
  // Little room either way: the side with more, capped to it.
  const tight = placeList({ top: 160, bottom: 196 }, { top: 60, bottom: 330 }, 212);
  expect(tight).toEqual({ upward: false, room: 330 - 196 - 6 - 8 });
});

it('keeps a change made while another one is saving', async () => {
  let release: (value: unknown) => void = () => {};
  invoke.mockImplementationOnce(
    (_cmd: string, args?: Record<string, unknown>) =>
      new Promise((resolve) => {
        // The answer reflects the file as it was then: gapless still on.
        const snapshot = { ...stored, ...((args as { patch: object }).patch ?? {}), gapless: true };
        release = () => resolve(snapshot);
      }),
  );
  const slow = updateSettings({ replayGain: 'track' });
  await updateSettings({ gapless: false });
  expect(settings.gapless).toBe(false);
  release(undefined);
  await slow;
  expect(settings.replayGain).toBe('track');
  expect(settings.gapless).toBe(false);
});

it('reverts only the failed keys', async () => {
  await updateSettings({ gapless: true, replayGain: 'album' });
  invoke.mockImplementationOnce(async () => {
    throw new Error('disk full');
  });
  const failing = updateSettings({ gapless: false });
  expect(settings.gapless).toBe(false);
  await expect(failing).rejects.toThrow('disk full');
  expect(settings.gapless).toBe(true);
  expect(settings.replayGain).toBe('album');
});

// Navidrome section. The store is set by hand: in the app `initNavidrome` fills it from the
// backend, here nothing listens.

const OFF: NavidromeStatus = {
  configured: false,
  url: null,
  user: null,
  state: 'off',
  lastSync: null,
  trackCount: 0,
  albumCount: 0,
  progress: null,
  error: null,
};

const DEMO_URL = 'http://127.0.0.1:4533/music';

function setStatus(patch: Partial<NavidromeStatus>) {
  Object.assign(navidrome, { ...OFF, ...patch });
  flushSync();
}

const connected = (patch: Partial<NavidromeStatus> = {}) =>
  setStatus({ configured: true, url: DEMO_URL, user: 'demo', state: 'online', ...patch });

function section() {
  return target.querySelector<HTMLElement>('section[aria-labelledby="settings-navidrome"]')!;
}

function field(name: 'url' | 'user' | 'password') {
  return section().querySelector<HTMLInputElement>(`input[name="navidrome-${name}"]`)!;
}

function type(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
}

function fillForm(password = 'demo-pass') {
  type(field('url'), DEMO_URL);
  type(field('user'), 'demo');
  type(field('password'), password);
}

function button(label: string) {
  const found = [...section().querySelectorAll<HTMLButtonElement>('button')].find(
    (b) => b.textContent?.trim() === label,
  );
  if (!found) throw new Error(`no button "${label}"`);
  return found;
}

function mountSettings() {
  mounted.push(mount(Settings, { target }));
  flushSync();
}

it('test_connection_shows_server_version', async () => {
  setStatus({});
  mountSettings();
  // The section sits between Library and Playback.
  const ids = [...target.querySelectorAll('section h2')].map((h) => h.id);
  expect(ids.slice(0, 3)).toEqual(['settings-library', 'settings-navidrome', 'settings-playback']);
  expect(field('url').placeholder).toBe('https://music.example.com');
  expect(field('password').type).toBe('password');

  fillForm();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('audio_devices', undefined));
  invoke.mockClear();
  invoke.mockImplementationOnce(async () => ({
    serverType: 'navidrome',
    serverVersion: '0.53.3',
    apiVersion: '1.16.1',
    openSubsonic: true,
  }));
  button('Test connection').click();
  await vi.waitFor(() => {
    flushSync();
    expect(section().querySelector('.result.ok')?.textContent).toContain('Connected to Navidrome 0.53.3 (API 1.16.1)');
  });
  expect(invoke).toHaveBeenCalledWith('navidrome_test', { url: DEMO_URL, user: 'demo', password: 'demo-pass' });
  // A test saves nothing and keeps the password for the connect that follows.
  expect(field('password').value).toBe('demo-pass');
  expect(invoke).not.toHaveBeenCalledWith('navidrome_connect', expect.anything());
});

it('wrong_credentials_show_the_mapped_message', async () => {
  setStatus({});
  mountSettings();
  fillForm('nope');
  invoke.mockImplementationOnce(async () => {
    throw { code: 'wrongCredentials', detail: null };
  });
  button('Test connection').click();
  await vi.waitFor(() => {
    flushSync();
    expect(section().querySelector('[role="alert"]')?.textContent).toBe('Wrong user name or password.');
  });

  // The keychain's length cap has its own text.
  invoke.mockImplementationOnce(async () => {
    throw { code: 'keychainTooLong', detail: 'username too long' };
  });
  button('Connect').click();
  await vi.waitFor(() => {
    flushSync();
    expect(section().querySelector('[role="alert"]')?.textContent).toBe(
      'The user name and server address are too long for the system keychain. Use a shorter user name or server address.',
    );
  });
});

it('connect_clears_the_password_field', async () => {
  setStatus({});
  mountSettings();
  fillForm();
  // A failed attempt clears it too.
  invoke.mockImplementationOnce(async () => {
    throw { code: 'unreachable', detail: 'connect timed out' };
  });
  button('Connect').click();
  await vi.waitFor(() => {
    flushSync();
    expect(section().querySelector('[role="alert"]')?.textContent).toContain('The server cannot be reached');
  });
  expect(field('password').value).toBe('');
  expect(field('url').value).toBe(DEMO_URL);
  // A failed connect asks for the password again, with the focus already there.
  await vi.waitFor(() => expect(document.activeElement).toBe(field('password')));

  type(field('password'), 'demo-pass');
  // The backend publishes the status (as `navidrome://status`) before the command returns.
  invoke.mockImplementationOnce(async () => {
    connected({ state: 'syncing' });
    return { ...OFF, configured: true, url: DEMO_URL, user: 'demo', state: 'syncing' };
  });
  button('Connect').click();
  await vi.waitFor(() => {
    flushSync();
    expect(section().querySelector('input[name="navidrome-password"]')).toBeNull();
  });
  expect(invoke).toHaveBeenCalledWith('navidrome_connect', { url: DEMO_URL, user: 'demo', password: 'demo-pass' });
  expect(section().textContent).toContain(DEMO_URL);
  expect(section().textContent).toContain('demo');
  expect(section().querySelector('.status')?.textContent).toContain('Syncing');
  // The focus lands on Sync now, which stays focusable (busy, not disabled) while syncing.
  await vi.waitFor(() => expect(document.activeElement).toBe(button('Sync now')));
  expect(button('Sync now').disabled).toBe(false);
  expect(button('Sync now').getAttribute('aria-disabled')).toBe('true');
});

it('a_status_event_before_connect_resolves_wins', async () => {
  setStatus({});
  mountSettings();
  fillForm();
  // The first cycle is quick: `online` arrives before the command's own (older) answer.
  invoke.mockImplementationOnce(async () => {
    connected({ state: 'syncing' });
    connected({ state: 'online', lastSync: Date.now(), trackCount: 600, albumCount: 60 });
    return { ...OFF, configured: true, url: DEMO_URL, user: 'demo', state: 'syncing' };
  });
  button('Connect').click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('navidrome_connect', expect.anything()));
  await vi.waitFor(() => expect(document.activeElement).toBe(button('Sync now')));
  flushSync();
  expect(navidrome.state).toBe('online');
  expect(section().querySelector('.status')?.textContent).toContain('Synced just now');
  expect(button('Sync now').getAttribute('aria-disabled')).toBe('false');
});

it('focus_stays_in_the_form_while_pending', async () => {
  setStatus({});
  mountSettings();
  fillForm();
  let fail: (reason: unknown) => void = () => {};
  invoke.mockImplementationOnce(() => new Promise((_, reject) => (fail = reject)));
  const connect = button('Connect');
  connect.focus();
  connect.click();
  flushSync();
  // Busy, but nothing is `disabled`: the focus stays where it was.
  expect(section().querySelector('.nd-feedback')?.textContent).toContain('Connecting...');
  expect(connect.disabled).toBe(false);
  expect(connect.getAttribute('aria-disabled')).toBe('true');
  for (const name of ['url', 'user', 'password'] as const) {
    expect(field(name).disabled).toBe(false);
    expect(field(name).readOnly).toBe(true);
    expect(field(name).getAttribute('aria-disabled')).toBe('true');
  }
  expect(document.activeElement).toBe(connect);
  // A second press while busy sends nothing.
  connect.click();
  expect(invoke.mock.calls.filter(([cmd]) => cmd === 'navidrome_connect')).toHaveLength(1);
  fail({ code: 'wrongCredentials', detail: null });
  await vi.waitFor(() => expect(document.activeElement).toBe(field('password')));
  expect(field('password').readOnly).toBe(false);
});

it('connected_shows_status_and_sync_now', async () => {
  connected({ lastSync: Date.now() - 4 * 60_000, trackCount: 31204, albumCount: 2100 });
  mountSettings();
  const status = () => section().querySelector('.status')!.textContent!;
  expect(status()).toContain('Synced 4 minutes ago');
  expect(status()).toContain('31,204 tracks');
  expect(status()).toContain('2,100 albums');
  expect(section().textContent).toMatch(/Windows Credential Manager|macOS Keychain/);
  expect(section().querySelector('input[name="navidrome-password"]')).toBeNull();

  invoke.mockClear();
  button('Sync now').click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('navidrome_sync', undefined));

  // One persistent live region says the state; the running counts stay out of it.
  const live = () => section().querySelector('.nd-live')!;
  expect(live().getAttribute('role')).toBe('status');
  expect(live().textContent).toBe('Online');

  connected({ state: 'syncing', progress: { done: 12000, total: 31204 }, trackCount: 12000, albumCount: 900 });
  expect(status()).toContain('Syncing 12,000 of 31,204 tracks...');
  // The count is in the line once, not repeated after it.
  expect(status()).not.toContain('12,000 tracks');
  expect(status()).not.toContain('900 albums');
  expect(button('Sync now').getAttribute('aria-disabled')).toBe('true');
  invoke.mockClear();
  button('Sync now').click();
  expect(invoke).not.toHaveBeenCalledWith('navidrome_sync', undefined);
  const bar = section().querySelector('[role="progressbar"]')!;
  expect(bar.getAttribute('aria-valuenow')).toBe('12000');
  expect(bar.getAttribute('aria-valuemax')).toBe('31204');
  expect(bar.querySelector('i')?.getAttribute('style')).toContain('scaleX(');
  expect(live().textContent).toBe('Syncing...');
  connected({ state: 'syncing', progress: { done: 13000, total: 31204 }, trackCount: 13000 });
  expect(live().textContent).toBe('Syncing...');
  // Without a total the bar is indeterminate.
  connected({ state: 'syncing', progress: null, trackCount: 600, albumCount: 60 });
  expect(section().querySelector('[role="progressbar"]')?.hasAttribute('aria-valuenow')).toBe(false);
  expect(status()).toContain('600 tracks, 60 albums');

  connected({ state: 'offline', trackCount: 31204 });
  expect(status()).toContain('Server not reachable, showing the saved library');
  expect(live().textContent).toBe('Server not reachable, showing the saved library');

  connected({ state: 'error', error: { code: 'syncRefused', detail: 'short page' }, trackCount: 31204 });
  const refused =
    'The server sent an incomplete library, so Prism kept the songs it already had. It tries again at the next sync.';
  expect(section().querySelector('.status .status-error')?.textContent).toBe(refused);
  expect(live().textContent).toBe(refused);
  expect(section().querySelector('.dot')?.classList.contains('error')).toBe(true);
});

it('disconnect_needs_the_confirm_row', async () => {
  connected();
  mountSettings();
  invoke.mockClear();
  const question = 'Sign out and remove the Navidrome library from Prism?';
  const disconnects = () =>
    [...section().querySelectorAll<HTMLButtonElement>('button')].filter((b) => b.textContent?.trim() === 'Disconnect');
  button('Disconnect').click();
  flushSync();
  expect(invoke).not.toHaveBeenCalledWith('navidrome_disconnect', undefined);
  expect(section().textContent).toContain(question);
  // The trigger hides while the row is open; the focus waits on Keep.
  expect(disconnects()).toHaveLength(1);
  expect(disconnects()[0].classList.contains('danger')).toBe(true);
  await vi.waitFor(() => expect(document.activeElement).toBe(button('Keep')));
  button('Keep').click();
  flushSync();
  expect(section().textContent).not.toContain(question);
  await vi.waitFor(() => expect(document.activeElement).toBe(button('Disconnect')));

  // Esc cancels too, and does not reach the window.
  button('Disconnect').click();
  await vi.waitFor(() => expect(document.activeElement).toBe(button('Keep')));
  const outer = vi.fn();
  window.addEventListener('keydown', outer);
  key(button('Keep'), 'Escape');
  window.removeEventListener('keydown', outer);
  expect(outer).not.toHaveBeenCalled();
  expect(section().textContent).not.toContain(question);
  await vi.waitFor(() => expect(document.activeElement).toBe(button('Disconnect')));

  button('Disconnect').click();
  flushSync();
  section().querySelector<HTMLButtonElement>('.link.danger')!.click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('navidrome_disconnect', undefined));
});

it('folder_remove_confirm_moves_the_focus_and_esc_keeps', async () => {
  mountSettings();
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelectorAll('.folder')).toHaveLength(2);
  });
  const row = () => target.querySelectorAll('.folder')[0];
  row().querySelector<HTMLButtonElement>('.remove-trigger')!.click();
  await vi.waitFor(() => expect(document.activeElement).toBe(row().querySelector('.keep')));
  key(row().querySelector('.keep')!, 'Escape');
  expect(row().querySelector('.link.danger')).toBeNull();
  await vi.waitFor(() => expect(document.activeElement).toBe(row().querySelector('.remove-trigger')));
  row().querySelector<HTMLButtonElement>('.remove-trigger')!.click();
  await vi.waitFor(() => expect(document.activeElement).toBe(row().querySelector('.keep')));
  row().querySelector<HTMLButtonElement>('.keep')!.click();
  await vi.waitFor(() => expect(document.activeElement).toBe(row().querySelector('.remove-trigger')));
  expect(invoke).not.toHaveBeenCalledWith('library_remove_folder', expect.anything());
});

it('quality_segmented_saves_navidromeQuality', async () => {
  await updateSettings({ navidromeQuality: 'original' });
  setStatus({});
  mountSettings();
  const options = [...section().querySelectorAll<HTMLButtonElement>('[role="radio"][data-quality]')];
  expect(options.map((o) => o.dataset.quality)).toEqual(['original', '320', '192', '128']);
  expect(options[0].getAttribute('aria-checked')).toBe('true');
  options[2].click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { navidromeQuality: '192' } }));
  flushSync();
  expect(options[2].getAttribute('aria-checked')).toBe('true');
});

it('password_is_never_prefilled', async () => {
  // A saved account whose password the server no longer takes: signing in again asks for
  // the password only, and it starts empty.
  connected({ state: 'error', error: { code: 'wrongCredentials', detail: null } });
  mountSettings();
  expect(section().querySelector('input[type="password"]')).toBeNull();
  button('Sign in again').click();
  flushSync();
  expect(field('password').value).toBe('');
  expect(field('password').getAttribute('autocomplete')).toBe('new-password');
  await vi.waitFor(() => expect(document.activeElement).toBe(field('password')));
  button('Cancel').click();
  flushSync();
  expect(section().querySelector('input[type="password"]')).toBeNull();
  await vi.waitFor(() => expect(document.activeElement).toBe(button('Sign in again')));

  setStatus({});
  expect(field('password').value).toBe('');
  expect(field('password').getAttribute('autocomplete')).toBe('new-password');
});

it('relogin_keeps_the_saved_account', async () => {
  // The tls text suggests http://, but a different address is a different server (and a
  // wipe): signing in again cannot change it. That goes through Disconnect.
  connected({ state: 'error', error: { code: 'tls', detail: null } });
  mountSettings();
  button('Sign in again').click();
  flushSync();
  expect(section().querySelector('input[name="navidrome-url"]')).toBeNull();
  expect(section().querySelector('input[name="navidrome-user"]')).toBeNull();
  expect(section().querySelector('.relogin')?.textContent).toContain(DEMO_URL);
  expect(section().querySelector('.relogin')?.textContent).toContain('Signed in as demo');

  type(field('password'), 'demo-pass');
  invoke.mockImplementationOnce(async () => {
    connected({ state: 'syncing' });
    return { ...OFF, configured: true, url: DEMO_URL, user: 'demo', state: 'syncing' };
  });
  button('Connect').click();
  await vi.waitFor(() =>
    expect(invoke).toHaveBeenCalledWith('navidrome_connect', { url: DEMO_URL, user: 'demo', password: 'demo-pass' }),
  );
  await vi.waitFor(() => {
    flushSync();
    expect(section().querySelector('input[type="password"]')).toBeNull();
  });
});

it('has the same defaults as the backend', () => {
  expect(DEFAULT_SETTINGS).toEqual({
    language: 'system',
    volume: 0.8,
    outputDevice: null,
    gapless: true,
    replayGain: 'off',
    crossfade: 'flow',
    crossfadeMs: 4000,
    coverLookup: true,
    miniOnTop: false,
    miniShortcut: 'CommandOrControl+Alt+P',
    startWithSystem: false,
    repeat: 'off',
    shuffle: false,
    navidromeUrl: null,
    navidromeUser: null,
    navidromeQuality: 'original',
  });
});
