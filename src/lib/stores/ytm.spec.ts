import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { YtmState } from '../types';

type Handler = (event: { event: string; id: number; payload: unknown }) => void;

const listeners: Array<{ event: string; handler: Handler }> = [];

/** Delivers an event to every listener of that name. */
const emit = (event: string, payload: unknown) =>
  listeners.filter((l) => l.event === event).forEach((l) => l.handler({ event, id: 1, payload }));
const count = (event: string) => listeners.filter((l) => l.event === event).length;
const unlisten = vi.fn();

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (event: string, handler: Handler) => {
    const entry = { event, handler };
    listeners.push(entry);
    return () => {
      unlisten(event);
      listeners.splice(listeners.indexOf(entry), 1);
    };
  }),
}));

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const { ytm, reportRect, hideView, signIn, signOut, initYtm, setOverlayOpen } = await import('./ytm.svelte');
const { player, initPlayer } = await import('./player.svelte');
const { openOverlay, closeOverlay, anyOverlay, setOverlayListener } = await import('./overlays.svelte');
// YouTubeView sets this while it is mounted; here the store is driven without it.
setOverlayListener(setOverlayOpen);

const idle: YtmState = {
  status: 'idle',
  title: '',
  artist: '',
  album: '',
  coverId: null,
  positionMs: 0,
  durationMs: 0,
  canNext: false,
  canPrev: false,
};
const rect = { x: 220, y: 40, width: 800, height: 500 };

let frames: Array<() => void> = [];

/** Runs every queued animation frame once. */
const flushFrames = () => {
  const run = frames;
  frames = [];
  run.forEach((cb) => cb());
};

const shows = () => invoke.mock.calls.filter((call) => call[0] === 'ytm_show');

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
  listeners.length = 0;
  unlisten.mockClear();
  frames = [];
  vi.stubGlobal('requestAnimationFrame', (cb: () => void) => frames.push(cb));
  vi.stubGlobal('cancelAnimationFrame', () => {
    frames = [];
  });
});

afterEach(async () => {
  hideView();
  for (const id of ['palette', 'menu', 'about']) closeOverlay(id);
  flushFrames();
  ytm.state = idle;
  player.active = 'prism';
  vi.unstubAllGlobals();
});

it('rect_reports_are_throttled_to_frames', () => {
  reportRect({ x: 1, y: 2, width: 300, height: 200 });
  reportRect({ x: 1, y: 2, width: 310, height: 200 });
  reportRect(rect);
  expect(shows()).toHaveLength(0);
  expect(ytm.shown).toBe(true);
  flushFrames();
  expect(shows()).toEqual([['ytm_show', { rect }]]);
});

it('does not resend an unchanged rect', () => {
  reportRect(rect);
  flushFrames();
  reportRect({ ...rect });
  flushFrames();
  expect(shows()).toHaveLength(1);
  reportRect({ ...rect, width: 801 });
  flushFrames();
  expect(shows()).toHaveLength(2);
});

it('hides the view and sends the rect again when it returns', () => {
  reportRect(rect);
  flushFrames();
  hideView();
  expect(ytm.shown).toBe(false);
  expect(invoke).toHaveBeenLastCalledWith('ytm_hide', undefined);
  reportRect(rect);
  flushFrames();
  expect(shows()).toHaveLength(2);
});

it('a frame that is queued when the view hides sends nothing', () => {
  reportRect(rect);
  hideView();
  flushFrames();
  expect(shows()).toHaveLength(0);
});

it('overlay_hides_the_ytm_view', () => {
  reportRect(rect);
  flushFrames();
  openOverlay('palette');
  expect(anyOverlay()).toBe(true);
  expect(ytm.hiddenForOverlay).toBe(true);
  expect(ytm.shown).toBe(true);
  expect(invoke).toHaveBeenLastCalledWith('ytm_hide', undefined);
  // A rect that arrives while the overlay is open is kept, not sent.
  reportRect({ ...rect, width: 700 });
  flushFrames();
  expect(shows()).toHaveLength(1);
});

it('last_overlay_closing_shows_it_again', () => {
  reportRect(rect);
  flushFrames();
  openOverlay('palette');
  openOverlay('menu');
  closeOverlay('palette');
  expect(ytm.hiddenForOverlay).toBe(true);
  expect(shows()).toHaveLength(1);
  closeOverlay('menu');
  expect(ytm.hiddenForOverlay).toBe(false);
  expect(anyOverlay()).toBe(false);
  expect(invoke).toHaveBeenLastCalledWith('ytm_show', { rect });
});

it('reopening after the overlay shows the rect that arrived meanwhile', () => {
  reportRect(rect);
  flushFrames();
  openOverlay('about');
  reportRect({ ...rect, width: 700 });
  flushFrames();
  closeOverlay('about');
  expect(invoke).toHaveBeenLastCalledWith('ytm_show', { rect: { ...rect, width: 700 } });
});

it('an overlay before the first show sends no hide and keeps the rect for the close', () => {
  reportRect(rect);
  openOverlay('palette');
  flushFrames();
  expect(ytm.hiddenForOverlay).toBe(true);
  expect(invoke).not.toHaveBeenCalled();
  closeOverlay('palette');
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(invoke).toHaveBeenLastCalledWith('ytm_show', { rect });
});

it('leaving a view that was never shown sends no hide', () => {
  reportRect(rect);
  hideView();
  expect(invoke).not.toHaveBeenCalled();
});

it('an overlay does nothing while the view is not shown', () => {
  openOverlay('palette');
  expect(ytm.hiddenForOverlay).toBe(false);
  expect(invoke).not.toHaveBeenCalled();
  closeOverlay('palette');
  expect(invoke).not.toHaveBeenCalled();
});

it('closing an overlay that was never opened is harmless', () => {
  reportRect(rect);
  flushFrames();
  closeOverlay('ghost');
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(ytm.hiddenForOverlay).toBe(false);
});

it('the view mounting while an overlay is open stays hidden until it closes', () => {
  openOverlay('palette');
  reportRect(rect);
  flushFrames();
  expect(ytm.shown).toBe(true);
  expect(ytm.hiddenForOverlay).toBe(true);
  expect(shows()).toHaveLength(0);
  closeOverlay('palette');
  expect(invoke).toHaveBeenLastCalledWith('ytm_show', { rect });
});

it('leaving the view while an overlay hides it does not hide twice or show later', () => {
  reportRect(rect);
  flushFrames();
  openOverlay('palette');
  hideView();
  expect(ytm.shown).toBe(false);
  expect(ytm.hiddenForOverlay).toBe(false);
  invoke.mockClear();
  closeOverlay('palette');
  expect(invoke).not.toHaveBeenCalled();
});

it('setOverlayOpen is what the registry drives', () => {
  reportRect(rect);
  flushFrames();
  setOverlayOpen(true);
  expect(ytm.hiddenForOverlay).toBe(true);
  setOverlayOpen(false);
  expect(ytm.hiddenForOverlay).toBe(false);
});

it('signIn and signOut use the pinned argument names', async () => {
  await signIn();
  await signOut();
  expect(invoke).toHaveBeenCalledWith('ytm_sign_in', undefined);
  expect(invoke).toHaveBeenCalledWith('ytm_sign_out', undefined);
});

it('active_player_mirrors_the_backend', async () => {
  const stopPlayer = await initPlayer();
  expect(player.active).toBe('prism');
  emit('transport://active', 'ytm');
  expect(player.active).toBe('ytm');
  emit('transport://active', 'prism');
  expect(player.active).toBe('prism');
  stopPlayer();
  expect(count('transport://active')).toBe(0);
});

it('follows ytm://state and loads the initial state', async () => {
  const loaded: YtmState = { ...idle, status: 'paused', title: 'Testlied' };
  invoke.mockImplementation(async (command: string) => (command === 'ytm_state' ? loaded : undefined));
  const stop = await initYtm();
  expect(ytm.state.title).toBe('Testlied');
  const playing: YtmState = { ...idle, status: 'playing', title: 'Neu', positionMs: 5, durationMs: 100 };
  emit('ytm://state', playing);
  expect(ytm.state).toEqual(playing);
  stop();
});

it('an event that arrives before the initial state wins', async () => {
  let answer: (state: YtmState) => void = () => {};
  invoke.mockImplementation((command: string) =>
    command === 'ytm_state' ? new Promise<YtmState>((resolve) => (answer = resolve)) : Promise.resolve(),
  );
  const pending = initYtm();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('ytm_state', undefined));
  const fresh: YtmState = { ...idle, status: 'playing', title: 'Neu' };
  emit('ytm://state', fresh);
  answer({ ...idle, title: 'Alt' });
  const stop = await pending;
  expect(ytm.state.title).toBe('Neu');
  stop();
});

it('a second init replaces the first one instead of doubling the listeners', async () => {
  await initYtm();
  const stop = await initYtm();
  expect(count('ytm://state')).toBe(1);
  stop();
  expect(listeners).toHaveLength(0);
});

it('cleanup removes the listeners', async () => {
  const stop = await initYtm();
  expect(count('ytm://state')).toBe(1);
  expect(count('transport://active')).toBe(0);
  stop();
  expect(listeners).toHaveLength(0);
  stop();
  expect(unlisten).toHaveBeenCalledTimes(1);
});
