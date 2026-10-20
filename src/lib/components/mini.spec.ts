import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { PlayerState, Track } from '../types';

const track: Track = {
  id: 'local:t1',
  source: 'local',
  title: 'Glass Tides',
  artist: 'Lumen Field',
  albumArtist: 'Lumen Field',
  album: 'Low Light',
  albumId: 'a1',
  trackNo: 1,
  discNo: 1,
  durationMs: 200_000,
  year: 2021,
  genre: 'Ambient',
  format: 'flac',
  sampleRate: 96_000,
  bitDepth: 24,
  bitrateKbps: null,
  coverId: null,
  path: 'C:/demo/1.flac',
  replayGainTrackDb: null,
  replayGainAlbumDb: null,
  replayGainTrackPeak: null,
  replayGainAlbumPeak: null,
};

let state: PlayerState;
let flowOutcome: unknown;

const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
  switch (cmd) {
    case 'player_state':
      return state;
    case 'favorites_ids':
      return { tracks: [], albums: [], artists: [] };
    case 'mini_layout':
      return { top: 300 };
    case 'flow_start':
      return flowOutcome ?? { outcome: 'started', variant: (args as { variant: unknown }).variant };
    default:
      return undefined;
  }
});
const emitTo = vi.fn(async (..._args: unknown[]) => {});
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
}));
type Handler = (event: { payload: unknown }) => void;
const handlers = new Map<string, Handler>();
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, handler: Handler) => {
    handlers.set(name, handler);
    return () => {
      if (handlers.get(name) === handler) handlers.delete(name);
    };
  }),
  emitTo: (...args: unknown[]) => emitTo(...args),
}));

const { default: MiniPlayer } = await import('./MiniPlayer.svelte');
const { setLang } = await import('../i18n');
const { DEFAULT_SETTINGS } = await import('../stores/settings.svelte');

let target: HTMLElement;
beforeEach(() => {
  setLang('en');
  invoke.mockClear();
  emitTo.mockClear();
  flowOutcome = undefined;
  state = {
    status: 'playing',
    track,
    positionMs: 76_000,
    durationMs: 200_000,
    volume: 0.8,
    queueIndex: 0,
    queueLength: 1,
    queueVersion: 1,
  };
  target = document.createElement('div');
  document.body.append(target);
});
afterEach(() => target.remove());

async function settle() {
  for (let i = 0; i < 6; i++) {
    await tick();
    await Promise.resolve();
  }
  flushSync();
}

it('shows the title, artist and album, and the source with the quality', async () => {
  const mini = mount(MiniPlayer, { target });
  await vi.waitFor(async () => {
    await settle();
    expect(target.querySelector('.title')?.textContent?.trim()).toBe('Glass Tides');
  });
  expect(target.querySelector('.sub')?.textContent?.trim()).toBe('Lumen Field · Low Light');
  expect(target.querySelector('.source')?.textContent?.trim()).toBe('Local files · FLAC 24/96');
  // Playing: the ring shows pause and the progress.
  expect(target.querySelector('.ring')?.getAttribute('aria-label')).toBe('Pause');
  unmount(mini);
});

it('opens Prism with the open button', async () => {
  const mini = mount(MiniPlayer, { target });
  await settle();
  const open = [...target.querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Open Prism');
  expect(open).toBeDefined();
  open!.click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('main_show', undefined));
  unmount(mini);
});

it('opens Now Playing in the main window from the cover', async () => {
  const mini = mount(MiniPlayer, { target });
  await vi.waitFor(async () => {
    await settle();
    expect(target.querySelector('.cover-button')).not.toBeNull();
  });
  target.querySelector<HTMLButtonElement>('.cover-button')!.click();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('main_show', undefined));
  expect(emitTo).toHaveBeenCalledWith('main', 'nav://go', { view: 'nowplaying' });
  unmount(mini);
});

it('idles with Flow and says what it does', async () => {
  state = { ...state, status: 'idle', track: null, positionMs: 0, durationMs: 0 };
  const mini = mount(MiniPlayer, { target });
  await settle();
  expect(target.querySelector('.ring .label')?.textContent).toBe('Flow');
  expect(target.querySelector('.title')?.textContent?.trim()).toBe('Ready when you are');
  unmount(mini);
});

it('handles an empty library inside the mini player', async () => {
  state = { ...state, status: 'idle', track: null, positionMs: 0, durationMs: 0 };
  flowOutcome = { outcome: 'empty' };
  const mini = mount(MiniPlayer, { target });
  await settle();
  target.querySelector<HTMLButtonElement>('.ring')!.click();
  await vi.waitFor(async () => {
    await settle();
    expect(target.querySelector('[role="status"]')?.textContent).toContain('Your library is empty');
  });
  expect(emitTo).not.toHaveBeenCalled();
  unmount(mini);
});

it('reads the settings again and closes the Flow menu when it is shown', async () => {
  const mini = mount(MiniPlayer, { target });
  await settle();
  const reads = () => invoke.mock.calls.filter((call) => call[0] === 'settings_get').length;
  const before = reads();
  target.querySelector('.ring')!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true }));
  await vi.waitFor(async () => {
    await settle();
    expect(document.querySelector('[role="menu"]')).not.toBeNull();
  });
  expect(invoke).toHaveBeenCalledWith('mini_menu', { open: true });
  handlers.get('mini://shown')!({ payload: null });
  await settle();
  expect(document.querySelector('[role="menu"]')).toBeNull();
  expect(invoke).toHaveBeenCalledWith('mini_menu', { open: false });
  expect(reads()).toBe(before + 1);
  unmount(mini);
});

it('follows a language change made in the main window', async () => {
  const mini = mount(MiniPlayer, { target });
  await settle();
  handlers.get('settings://changed')!({
    payload: { ...DEFAULT_SETTINGS, language: 'de' },
  });
  await settle();
  expect([...target.querySelectorAll('button')].some((b) => b.textContent?.trim() === 'Prism öffnen')).toBe(true);
  unmount(mini);
});
