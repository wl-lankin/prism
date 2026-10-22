import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { UpdateState } from '../types';

const IDLE: UpdateState = {
  status: 'idle',
  version: null,
  notes: null,
  progress: null,
  lastCheck: null,
  installsOnQuit: false,
};
let backend: UpdateState = { ...IDLE };
let restartFails = false;

const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
  switch (cmd) {
    case 'update_state':
      return backend;
    case 'update_restart':
      if (restartFails) throw new Error('signature mismatch');
      return undefined;
    case 'library_folders':
    case 'audio_devices':
      return [];
    case 'settings_set':
      return { ...(args as { patch: object }).patch };
    default:
      return undefined;
  }
});
vi.mock('@tauri-apps/api/core', () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args) }));
const listeners = new Map<string, (event: { payload: unknown }) => void>();
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, cb: (event: { payload: unknown }) => void) => {
    listeners.set(name, cb);
    return () => listeners.delete(name);
  }),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(async () => null) }));
vi.mock('@tauri-apps/api/app', () => ({ getVersion: async () => '1.0.0' }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(async () => {}) }));

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= function () {};

const { default: TransportStrip } = await import('./TransportStrip.svelte');
const { default: Settings } = await import('./Settings.svelte');
const { update, initUpdate } = await import('../stores/update.svelte');
const { settings } = await import('../stores/settings.svelte');
const { notice } = await import('../stores/notice.svelte');
const { setLang } = await import('../i18n');

let target: HTMLElement;
const mounted: ReturnType<typeof mount>[] = [];

beforeEach(() => {
  setLang('en');
  invoke.mockClear();
  listeners.clear();
  backend = { ...IDLE };
  restartFails = false;
  Object.assign(update, IDLE);
  settings.autoUpdate = true;
  notice.text = null;
  target = document.createElement('div');
  document.body.append(target);
});

afterEach(() => {
  for (const app of mounted.splice(0)) unmount(app);
  target.remove();
});

async function settle() {
  for (let i = 0; i < 6; i++) {
    await tick();
    await Promise.resolve();
  }
  flushSync();
}

function emit(state: UpdateState) {
  listeners.get('update://state')?.({ payload: state });
  flushSync();
}

const ready: UpdateState = {
  status: 'ready',
  version: '1.0.1',
  notes: 'Fixes for Testhausen\nFaster scans',
  progress: null,
  lastCheck: Date.now(),
  installsOnQuit: true,
};

describe('update store', () => {
  it('loads the state, then follows update://state', async () => {
    backend = { ...IDLE, status: 'upToDate', lastCheck: 1 };
    const stop = await initUpdate();
    expect(invoke).toHaveBeenCalledWith('update_state', undefined);
    expect(update.status).toBe('upToDate');
    emit(ready);
    expect(update.status).toBe('ready');
    expect(update.version).toBe('1.0.1');
    stop();
  });

  it('announces an update found with automatic updates off once', async () => {
    const stop = await initUpdate();
    const available: UpdateState = { ...ready, status: 'available' };
    emit(available);
    expect(notice.text).toContain('Prism 1.0.1 is available');
    notice.text = null;
    emit({ ...available, lastCheck: Date.now() + 1 });
    expect(notice.text).toBeNull();
    stop();
  });
});

describe('restart pill in the transport strip', () => {
  it('shows only when an update is ready', async () => {
    mounted.push(mount(TransportStrip, { target }));
    await settle();
    expect(target.querySelector('.update-pill')).toBeNull();
    for (const status of ['checking', 'available', 'downloading', 'failed', 'upToDate'] as const) {
      Object.assign(update, { ...ready, status });
      flushSync();
      expect(target.querySelector('.update-pill'), status).toBeNull();
    }
    Object.assign(update, ready);
    flushSync();
    const pill = target.querySelector<HTMLButtonElement>('.update-pill')!;
    expect(pill.textContent?.trim()).toBe('Restart to update');
    expect(pill.title).toBe('Install Prism 1.0.1 and restart');
    // In the right group, ahead of everything already there; no new divider.
    expect(pill.parentElement?.classList.contains('right')).toBe(true);
    expect(pill.parentElement?.firstElementChild).toBe(pill);
    expect(target.querySelector('.strip')?.querySelectorAll('hr, .divider')).toHaveLength(0);
  });

  it('installs and restarts on click, with a calm notice when that fails', async () => {
    Object.assign(update, ready);
    mounted.push(mount(TransportStrip, { target }));
    await settle();
    target.querySelector<HTMLButtonElement>('.update-pill')!.click();
    await settle();
    expect(invoke).toHaveBeenCalledWith('update_restart', undefined);
    expect(notice.text).toBeNull();

    restartFails = true;
    target.querySelector<HTMLButtonElement>('.update-pill')!.click();
    await settle();
    expect(notice.text).toBe('The update could not be installed. Prism stays as it is.');
  });

  it('speaks German', async () => {
    setLang('de');
    Object.assign(update, ready);
    mounted.push(mount(TransportStrip, { target }));
    await settle();
    expect(target.querySelector('.update-pill')?.textContent?.trim()).toBe('Zum Aktualisieren neu starten');
  });
});

describe('Settings > Updates', () => {
  function section() {
    return target.querySelector<HTMLElement>('#settings-section-updates')!;
  }

  async function mountSettings() {
    mounted.push(mount(Settings, { target }));
    await vi.waitFor(() => {
      flushSync();
      expect(section().textContent).toContain('Prism 1.0.0');
    });
  }

  it('shows the version, the last check and checks now', async () => {
    await mountSettings();
    expect(section().querySelector('h2')?.textContent).toBe('Updates');
    expect(section().textContent).toContain('Not checked yet');
    const check = [...section().querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Check now')!;
    check.click();
    await settle();
    expect(invoke).toHaveBeenCalledWith('update_check', undefined);

    Object.assign(update, { ...IDLE, status: 'checking' });
    flushSync();
    expect(check.disabled).toBe(true);
    expect(section().querySelector('.update-status')?.textContent).toBe('Checking for updates...');

    Object.assign(update, { ...IDLE, status: 'upToDate', lastCheck: Date.now() });
    flushSync();
    expect(section().querySelector('.update-status')?.textContent).toBe('Prism is up to date.');
    expect(section().textContent).toContain('Last checked just now');
  });

  it('toggles automatic updates', async () => {
    await mountSettings();
    const toggle = section().querySelector<HTMLButtonElement>('[role="switch"]')!;
    expect(toggle.getAttribute('aria-checked')).toBe('true');
    expect(document.getElementById(toggle.getAttribute('aria-labelledby')!)?.textContent).toBe(
      'Install updates automatically',
    );
    toggle.click();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('settings_set', { patch: { autoUpdate: false } }));
    flushSync();
    expect(toggle.getAttribute('aria-checked')).toBe('false');
  });

  it('offers the download of an announced update', async () => {
    Object.assign(update, { ...ready, status: 'available' });
    await mountSettings();
    expect(section().querySelector('.update-status')?.textContent).toBe('Prism 1.0.1 is available.');
    const download = [...section().querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Download')!;
    download.click();
    await settle();
    expect(invoke).toHaveBeenCalledWith('update_check', undefined);
  });

  it('shows the progress, the failure and the release notes', async () => {
    Object.assign(update, { ...ready, status: 'downloading', progress: 0.42 });
    await mountSettings();
    expect(section().querySelector('.update-status')?.textContent).toBe('Downloading Prism 1.0.1, 42%');
    expect(section().querySelector('.release-notes')?.textContent).toBe('Fixes for Testhausen\nFaster scans');
    expect(section().textContent).toContain('What is new in 1.0.1');

    Object.assign(update, { ...IDLE, status: 'failed', lastCheck: Date.now() });
    flushSync();
    expect(section().querySelector('.update-status')?.textContent).toBe(
      'The update could not be loaded. Prism stays as it is and tries again later.',
    );
    expect(section().querySelector('.release-notes')).toBeNull();
  });

  it('has the restart pill when ready', async () => {
    Object.assign(update, ready);
    await mountSettings();
    expect(section().querySelector('.update-status')?.textContent).toBe(
      'Prism 1.0.1 is ready. It installs when you quit Prism.',
    );
    const restart = [...section().querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Restart to update')!;
    restart.click();
    await settle();
    expect(invoke).toHaveBeenCalledWith('update_restart', undefined);
  });

  it('says a restart installs it when quitting would not', async () => {
    Object.assign(update, { ...ready, installsOnQuit: false });
    await mountSettings();
    expect(section().querySelector('.update-status')?.textContent).toBe(
      'Prism 1.0.1 is ready. Restart Prism to install it.',
    );
    expect([...section().querySelectorAll('button')].some((b) => b.textContent?.trim() === 'Restart to update')).toBe(
      true,
    );
    // The backend sends a new state when the setting changes.
    const stop = await initUpdate();
    emit({ ...ready, installsOnQuit: true });
    expect(section().querySelector('.update-status')?.textContent).toBe(
      'Prism 1.0.1 is ready. It installs when you quit Prism.',
    );
    stop();

    setLang('de');
    Object.assign(update, { ...ready, installsOnQuit: false });
    flushSync();
    expect(section().querySelector('.update-status')?.textContent).toBe(
      'Prism 1.0.1 ist bereit. Starte Prism neu, um es zu installieren.',
    );
  });

  it('speaks German', async () => {
    setLang('de');
    Object.assign(update, { ...IDLE, status: 'upToDate', lastCheck: Date.now() });
    mounted.push(mount(Settings, { target }));
    await vi.waitFor(() => {
      flushSync();
      expect(section().textContent).toContain('Prism 1.0.0');
    });
    expect(section().textContent).toContain('Updates automatisch installieren');
    expect(section().textContent).toContain('Zuletzt geprüft gerade eben');
    expect(section().textContent).toContain('Jetzt prüfen');
    expect(section().querySelector('.update-status')?.textContent).toBe('Prism ist auf dem neuesten Stand.');
  });
});
