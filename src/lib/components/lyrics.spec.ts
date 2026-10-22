import { flushSync, mount, tick, unmount, type Component } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Lyrics, LyricsAnswer, Track } from '../types';

type Ask = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
let lyricsAnswer: LyricsAnswer = { status: 'none' };
let lyricsDelay: Promise<void> | null = null;
const invoke = vi.fn<Ask>(async (cmd) => {
  if (cmd === 'lyrics_for_track' || cmd === 'lyrics_for_youtube') {
    if (lyricsDelay) await lyricsDelay;
    return lyricsAnswer;
  }
  if (cmd === 'player_state') {
    return { status: 'idle', track: null, positionMs: 0, durationMs: 0, volume: 0.8, queueIndex: 0, queueLength: 0, queueVersion: 0 };
  }
  if (cmd === 'player_queue_range') return [];
  if (cmd === 'favorites_ids') return { tracks: [], albums: [], artists: [] };
  if (cmd === 'window_vibrancy') return false;
  return undefined;
});
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
  convertFileSrc: (path: string) => path,
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(async () => null) }));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isMaximized: async () => false,
    onResized: async () => () => {},
    onDragDropEvent: async () => () => {},
  }),
}));

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= function () {};

const { default: App } = await import('../../App.svelte');
const { default: LyricsView } = await import('./LyricsView.svelte');
const { default: LyricsPanel } = await import('./LyricsPanel.svelte');
const { default: NowPlaying } = await import('./NowPlaying.svelte');
const { default: TransportStrip } = await import('./TransportStrip.svelte');
const { player } = await import('../stores/player.svelte');
const { ytm, applyState } = await import('../stores/ytm.svelte');
const { settings } = await import('../stores/settings.svelte');
const { nav } = await import('../stores/nav.svelte');
const { lyricsState } = await import('../stores/lyrics.svelte');
const { lyricsPanel, queuePanel, nowPlayingSide, toggleLyricsPanel, toggleQueuePanel } = await import('../stores/queue.svelte');
const { overlayOpen } = await import('../stores/overlays.svelte');
const { setLang } = await import('../i18n');

function track(id: string, over: Partial<Track> = {}): Track {
  return {
    id,
    source: 'local',
    title: `Song ${id}`,
    artist: 'Lumen Field',
    albumArtist: 'Lumen Field',
    album: 'Low Light',
    albumId: 'a1',
    trackNo: 1,
    discNo: 1,
    durationMs: 200_000,
    year: 2021,
    genre: null,
    format: 'flac',
    sampleRate: 44_100,
    bitDepth: 16,
    bitrateKbps: null,
    coverId: null,
    path: `C:/demo/${id}.flac`,
    replayGainTrackDb: null,
    replayGainAlbumDb: null,
    replayGainTrackPeak: null,
    replayGainAlbumPeak: null,
    ...over,
  };
}

const synced: Lyrics = {
  source: 'lrclib',
  synced: true,
  instrumental: false,
  lines: [
    { startMs: 0, text: 'First invented line' },
    { startMs: 2000, text: 'Second invented line' },
    { startMs: 4000, text: 'Third invented line' },
    { startMs: 6000, text: 'Fourth invented line' },
  ],
};
const plain: Lyrics = {
  source: 'navidrome',
  synced: false,
  instrumental: false,
  lines: [
    { startMs: null, text: 'Plain invented line' },
    { startMs: null, text: '' },
    { startMs: null, text: 'Another plain line' },
  ],
};

const ytmPlaying = {
  status: 'playing' as const,
  title: 'Glass Tides',
  artist: 'Lumen Field',
  album: 'Low Light',
  coverId: null,
  positionMs: 0,
  durationMs: 200_000,
  canNext: false,
  canPrev: false,
};

let target: HTMLElement;
const mounted: ReturnType<typeof mount>[] = [];

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function show(component: Component<any>, props: Record<string, unknown> = {}) {
  const app = mount(component, { target, props });
  mounted.push(app);
  flushSync();
  return app;
}

async function settle() {
  for (let i = 0; i < 6; i++) {
    await tick();
    await Promise.resolve();
  }
  flushSync();
}

const calls = (cmd: string) => invoke.mock.calls.filter((call) => call[0] === cmd);
const lines = () => [...target.querySelectorAll<HTMLElement>('button.line')];
const activeText = () => target.querySelector('[aria-current="true"]')?.textContent ?? null;

function play(id: string, positionMs = 0, status: 'playing' | 'paused' = 'paused') {
  Object.assign(player, { active: 'prism', status, track: track(id), positionMs, durationMs: 200_000 });
}

beforeEach(() => {
  setLang('en');
  invoke.mockClear();
  localStorage.clear();
  lyricsAnswer = { status: 'found', lyrics: synced };
  lyricsDelay = null;
  settings.lyricsLookup = true;
  queuePanel.open = false;
  lyricsPanel.open = false;
  nowPlayingSide.view = 'queue';
  lyricsState.key = null;
  lyricsState.view = { kind: 'idle' };
  nav.view = 'home';
  Object.assign(player, { active: 'prism', status: 'idle', track: null, positionMs: 0, durationMs: 0, queueLength: 0 });
  target = document.createElement('div');
  document.body.append(target);
});

afterEach(() => {
  vi.useRealTimers();
  applyState({ ...ytm.state, status: 'idle' });
  for (const app of mounted.splice(0)) unmount(app);
  target.remove();
});

describe('active line', () => {
  it('lights the last line that started, 150 ms early', async () => {
    play('a', 0);
    show(LyricsView);
    await settle();
    expect(calls('lyrics_for_track')).toEqual([['lyrics_for_track', { trackId: 'a' }]]);
    expect(lines()).toHaveLength(4);
    expect(activeText()).toBe('First invented line');
    player.positionMs = 1849;
    flushSync();
    expect(activeText()).toBe('First invented line');
    player.positionMs = 1850;
    flushSync();
    expect(activeText()).toBe('Second invented line');
    expect(lines()[0].classList.contains('past')).toBe(true);
  });

  it('runs on between engine reports while playing and stands still while paused', async () => {
    vi.useFakeTimers({ toFake: ['requestAnimationFrame', 'cancelAnimationFrame', 'performance', 'setTimeout', 'clearTimeout'] });
    play('a', 0, 'playing');
    show(LyricsView);
    await settle();
    expect(activeText()).toBe('First invented line');
    await vi.advanceTimersByTimeAsync(2100);
    expect(activeText()).toBe('Second invented line');
    await vi.advanceTimersByTimeAsync(2000);
    expect(activeText()).toBe('Third invented line');

    player.status = 'paused';
    player.positionMs = 4100;
    flushSync();
    await vi.advanceTimersByTimeAsync(5000);
    expect(activeText()).toBe('Third invented line');
  });

  it('follows YouTube Music from its drawn position', async () => {
    vi.useFakeTimers({ toFake: ['requestAnimationFrame', 'cancelAnimationFrame', 'performance', 'setTimeout', 'clearTimeout'] });
    player.active = 'ytm';
    applyState({
      status: 'playing',
      title: 'Glass Tides',
      artist: 'Lumen Field',
      album: 'Low Light',
      coverId: null,
      positionMs: 4200,
      durationMs: 200_000,
      canNext: false,
      canPrev: false,
    });
    show(LyricsView);
    await settle();
    await vi.advanceTimersByTimeAsync(300);
    expect(calls('lyrics_for_youtube')).toHaveLength(1);
    expect(calls('lyrics_for_track')).toHaveLength(0);
    expect(activeText()).toBe('Third invented line');
    await vi.advanceTimersByTimeAsync(1900);
    expect(activeText()).toBe('Fourth invented line');
    applyState({ ...ytm.state, status: 'idle' });
  });

  it('does not jump ahead by the length of a pause when playback resumes', async () => {
    vi.useFakeTimers({ toFake: ['requestAnimationFrame', 'cancelAnimationFrame', 'performance', 'setTimeout', 'clearTimeout'] });
    play('a', 1000, 'playing');
    show(LyricsView);
    await settle();
    await vi.advanceTimersByTimeAsync(100);
    player.status = 'paused';
    flushSync();
    await vi.advanceTimersByTimeAsync(30_000);
    player.status = 'playing';
    flushSync();
    await vi.advanceTimersByTimeAsync(20);
    expect(activeText()).toBe('First invented line');
  });

  it('keeps the clicked line on YouTube Music while its ticker runs', async () => {
    vi.useFakeTimers({ toFake: ['requestAnimationFrame', 'cancelAnimationFrame', 'performance', 'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] });
    player.active = 'ytm';
    applyState({ ...ytmPlaying, positionMs: 0 });
    show(LyricsView);
    await settle();
    await vi.advanceTimersByTimeAsync(300);
    lines()[2].click();
    flushSync();
    await vi.advanceTimersByTimeAsync(250);
    expect(activeText()).toBe('Third invented line');
  });

  it('holds a seek until a report near it arrives, so the highlight does not bounce', async () => {
    vi.useFakeTimers({ toFake: ['requestAnimationFrame', 'cancelAnimationFrame', 'performance', 'setTimeout', 'clearTimeout'] });
    play('a', 0, 'playing');
    show(LyricsView);
    await settle();
    lines()[2].click();
    flushSync();
    await vi.advanceTimersByTimeAsync(100);
    player.positionMs = 300;
    flushSync();
    await vi.advanceTimersByTimeAsync(50);
    expect(activeText()).toBe('Third invented line');
    await vi.advanceTimersByTimeAsync(700);
    player.positionMs = 350;
    flushSync();
    expect(activeText()).toBe('First invented line');
  });

  it('seeks to the start of a clicked line', async () => {
    play('a', 0);
    show(LyricsView);
    await settle();
    lines()[2].click();
    flushSync();
    expect(calls('player_seek')).toEqual([['player_seek', { ms: 4000 }]]);
    expect(activeText()).toBe('Third invented line');
  });
});

describe('following', () => {
  it('stops for 5 s after the user scrolls and comes back on its own', async () => {
    vi.useFakeTimers({ toFake: ['requestAnimationFrame', 'cancelAnimationFrame', 'performance', 'setTimeout', 'clearTimeout'] });
    play('a', 0);
    show(LyricsView);
    await settle();
    const scroller = target.querySelector<HTMLElement>('.scroll')!;
    const scrollTo = vi.fn();
    scroller.scrollTo = scrollTo as never;
    expect(target.querySelector('.back')).toBeNull();

    scroller.dispatchEvent(new WheelEvent('wheel', { bubbles: true }));
    flushSync();
    expect(target.querySelector('.back')?.textContent).toBe('Back to current line');
    player.positionMs = 2500;
    flushSync();
    expect(scrollTo).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(4900);
    expect(target.querySelector('.back')).not.toBeNull();
    await vi.advanceTimersByTimeAsync(200);
    flushSync();
    expect(target.querySelector('.back')).toBeNull();
    expect(scrollTo).toHaveBeenCalled();
  });

  it('comes back at once with the button, and touch scrolling stops it too', async () => {
    play('a', 0);
    show(LyricsView);
    await settle();
    const scroller = target.querySelector<HTMLElement>('.scroll')!;
    const scrollTo = vi.fn();
    scroller.scrollTo = scrollTo as never;
    scroller.dispatchEvent(new TouchEvent('touchmove', { bubbles: true }));
    flushSync();
    target.querySelector<HTMLElement>('.back')!.click();
    flushSync();
    expect(target.querySelector('.back')).toBeNull();
    expect(scrollTo).toHaveBeenCalled();
  });

  it('keeps the view still for a click on a line, even with a little jitter', async () => {
    play('a', 0);
    show(LyricsView);
    await settle();
    const move = (clientY: number) =>
      lines()[1].dispatchEvent(new PointerEvent('pointermove', { bubbles: true, buttons: 1, clientY }));
    lines()[1].dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, buttons: 1, clientY: 100 }));
    move(102);
    lines()[1].click();
    flushSync();
    expect(target.querySelector('.back')).toBeNull();
    move(110);
    flushSync();
    expect(target.querySelector('.back')).not.toBeNull();
  });

  it('stops following for keyboard scrolling', async () => {
    play('a', 0);
    show(LyricsView);
    await settle();
    target.querySelector('.scroll')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'PageDown', bubbles: true }));
    flushSync();
    expect(target.querySelector('.back')).not.toBeNull();
  });

  it('has one tab stop, and the arrow keys move between lines', async () => {
    play('a', 2500);
    show(LyricsView);
    await settle();
    const stops = () => lines().filter((line) => line.tabIndex === 0);
    expect(stops()).toEqual([lines()[1]]);
    lines()[1].focus();
    lines()[1].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    flushSync();
    expect(document.activeElement).toBe(lines()[2]);
    expect(stops()).toEqual([lines()[2]]);
    expect(target.querySelector('.back')).not.toBeNull();
  });

  it('scrolls without animation under reduced motion', async () => {
    vi.stubGlobal('matchMedia', () => ({ matches: true }));
    try {
      play('a', 2500);
      show(LyricsView);
      await settle();
      const scroller = target.querySelector<HTMLElement>('.scroll')!;
      const scrollTo = vi.fn();
      scroller.scrollTo = scrollTo as never;
      player.positionMs = 4500;
      flushSync();
      expect(scrollTo).not.toHaveBeenCalled();
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

describe('states', () => {
  it('says nothing plays', async () => {
    show(LyricsView);
    await settle();
    expect(target.textContent).toContain('Nothing is playing.');
    expect(invoke).not.toHaveBeenCalledWith('lyrics_for_track', expect.anything());
  });

  it('says lyrics are not available for radio and asks nothing', async () => {
    Object.assign(player, { status: 'playing', track: track('radio:1', { source: 'radio', durationMs: 0 }) });
    show(LyricsView);
    await settle();
    expect(target.textContent).toContain('Lyrics are not available for radio.');
    expect(calls('lyrics_for_track')).toHaveLength(0);
  });

  it('shows loading while the answer is on its way', async () => {
    let release = () => {};
    lyricsDelay = new Promise((resolve) => (release = resolve));
    play('a');
    show(LyricsView);
    await settle();
    expect(target.textContent).toContain('Loading lyrics');
    release();
    await settle();
    expect(target.textContent).not.toContain('Loading lyrics');
    expect(lines()).toHaveLength(4);
  });

  it('shows none, and the hint with a link to settings when the online search is off', async () => {
    lyricsAnswer = { status: 'none' };
    play('a');
    show(LyricsView);
    await settle();
    expect(target.textContent).toContain('No lyrics found');
    expect(target.textContent).not.toContain('Online search is off');

    settings.lyricsLookup = false;
    await settle();
    expect(target.textContent).toContain('No lyrics found');
    expect(target.textContent).toContain('Online search is off');
    [...target.querySelectorAll('button')].find((b) => b.textContent === 'Open settings')!.click();
    flushSync();
    expect(nav.view).toBe('settings');
  });

  it('shows an instrumental track', async () => {
    lyricsAnswer = { status: 'found', lyrics: { source: 'lrclib', synced: false, instrumental: true, lines: [] } };
    play('a');
    show(LyricsView);
    await settle();
    expect(target.textContent).toContain('Instrumental');
    expect(lines()).toHaveLength(0);
  });

  it('offers to try again after an error, and asks again', async () => {
    lyricsAnswer = { status: 'error' };
    play('a');
    show(LyricsView);
    await settle();
    expect(target.textContent).toContain('The lyrics could not be loaded.');
    lyricsAnswer = { status: 'found', lyrics: synced };
    [...target.querySelectorAll('button')].find((b) => b.textContent === 'Try again')!.click();
    await settle();
    expect(calls('lyrics_for_track')).toHaveLength(2);
    expect(lines()).toHaveLength(4);
  });

  it('treats a failing command as an error', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    invoke.mockImplementationOnce(async () => {
      throw 'not allowed';
    });
    play('a');
    show(LyricsView);
    await settle();
    expect(target.textContent).toContain('The lyrics could not be loaded.');
  });

  it('shows plain lyrics without a highlight, with the note and the source', async () => {
    lyricsAnswer = { status: 'found', lyrics: plain };
    play('a', 3000, 'playing');
    show(LyricsView);
    await settle();
    expect(target.textContent).toContain('Not synced');
    expect(target.textContent).toContain('Plain invented line');
    expect(target.textContent).toContain('Another plain line');
    expect(target.querySelector('[aria-current]')).toBeNull();
    expect(lines()).toHaveLength(0);
    expect(target.querySelector('.source')?.textContent).toBe('Lyrics: from Navidrome');
  });

  it('names the source of synced lyrics, in German too', async () => {
    play('a');
    show(LyricsView);
    await settle();
    expect(target.querySelector('.source')?.textContent).toBe('Lyrics: LRCLIB');
    setLang('de');
    flushSync();
    expect(target.querySelector('.source')?.textContent).toBe('Songtext: LRCLIB');
  });

  it('renders lyrics as text, never as HTML', async () => {
    lyricsAnswer = {
      status: 'found',
      lyrics: { ...synced, lines: [{ startMs: 0, text: '<img src=x onerror=alert(1)> invented' }] },
    };
    play('a');
    show(LyricsView);
    await settle();
    expect(target.querySelector('.scroll img')).toBeNull();
    expect(lines()[0].textContent).toBe('<img src=x onerror=alert(1)> invented');
  });
});

describe('one answer per track', () => {
  it('does not ask again when the view is shown again', async () => {
    play('a');
    const first = show(LyricsView);
    await settle();
    unmount(first);
    mounted.splice(mounted.indexOf(first), 1);
    show(LyricsView);
    await settle();
    expect(calls('lyrics_for_track')).toHaveLength(1);
    expect(lines()).toHaveLength(4);
  });

  it('asks again for the next track and drops the old answer', async () => {
    play('a');
    show(LyricsView);
    await settle();
    lyricsAnswer = { status: 'none' };
    play('b');
    await settle();
    expect(calls('lyrics_for_track').map((call) => call[1])).toEqual([{ trackId: 'a' }, { trackId: 'b' }]);
    expect(target.textContent).toContain('No lyrics found');
    expect(lyricsState.key).toBe('track:b');
  });

  it('ignores a late answer for a track that no longer plays', async () => {
    let release = () => {};
    lyricsDelay = new Promise((resolve) => (release = resolve));
    play('a');
    show(LyricsView);
    await settle();
    lyricsDelay = null;
    lyricsAnswer = { status: 'none' };
    play('b');
    await settle();
    expect(target.textContent).toContain('No lyrics found');
    lyricsAnswer = { status: 'found', lyrics: synced };
    release();
    await settle();
    expect(target.textContent).toContain('No lyrics found');
    expect(lines()).toHaveLength(0);
  });

  it('asks nothing for half-updated YouTube Music metadata and once for the settled song', async () => {
    vi.useFakeTimers({ toFake: ['requestAnimationFrame', 'cancelAnimationFrame', 'performance', 'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] });
    player.active = 'ytm';
    applyState({ ...ytmPlaying, durationMs: 0 });
    show(LyricsView);
    await settle();
    await vi.advanceTimersByTimeAsync(1000);
    expect(calls('lyrics_for_youtube')).toHaveLength(0);
    applyState({ ...ytmPlaying, artist: '' });
    await vi.advanceTimersByTimeAsync(1000);
    expect(calls('lyrics_for_youtube')).toHaveLength(0);

    applyState({ ...ytmPlaying, durationMs: 200_400 });
    await vi.advanceTimersByTimeAsync(299);
    expect(calls('lyrics_for_youtube')).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(2);
    expect(calls('lyrics_for_youtube')).toHaveLength(1);
    expect(lyricsState.key).toBe('ytm:Glass Tides|Lumen Field|200');

    // A song change: the new title arrives first, the artist 100 ms later.
    applyState({ ...ytmPlaying, title: 'Other Song', durationMs: 180_000 });
    await vi.advanceTimersByTimeAsync(100);
    applyState({ ...ytmPlaying, title: 'Other Song', artist: 'Prism Band', durationMs: 180_000 });
    await vi.advanceTimersByTimeAsync(400);
    expect(calls('lyrics_for_youtube')).toHaveLength(2);
    expect(lyricsState.key).toBe('ytm:Other Song|Prism Band|180');
  });

  it('keeps a found answer when only the online search is switched', async () => {
    play('a');
    show(LyricsView);
    await settle();
    settings.lyricsLookup = false;
    await settle();
    settings.lyricsLookup = true;
    await settle();
    expect(calls('lyrics_for_track')).toHaveLength(1);
    expect(lines()).toHaveLength(4);
  });

  it('asks again after the online search is switched on', async () => {
    settings.lyricsLookup = false;
    lyricsAnswer = { status: 'none' };
    play('a');
    show(LyricsView);
    await settle();
    lyricsAnswer = { status: 'found', lyrics: synced };
    settings.lyricsLookup = true;
    await settle();
    expect(calls('lyrics_for_track')).toHaveLength(2);
    expect(lines()).toHaveLength(4);
  });
});

describe('the side slot', () => {
  it('holds the queue or the lyrics, never both', () => {
    toggleQueuePanel(true);
    expect(queuePanel.open).toBe(true);
    toggleLyricsPanel(true);
    expect(lyricsPanel.open).toBe(true);
    expect(queuePanel.open).toBe(false);
    toggleQueuePanel(true);
    expect(queuePanel.open).toBe(true);
    expect(lyricsPanel.open).toBe(false);
    toggleQueuePanel(false);
    expect(queuePanel.open).toBe(false);
    expect(lyricsPanel.open).toBe(false);
  });

  it('asks nothing while the lyrics panel is closed, and registers as an overlay while open', async () => {
    play('a');
    show(LyricsPanel);
    await settle();
    expect(calls('lyrics_for_track')).toHaveLength(0);
    expect(overlayOpen('lyrics')).toBe(false);
    toggleLyricsPanel(true);
    await settle();
    expect(calls('lyrics_for_track')).toHaveLength(1);
    expect(overlayOpen('lyrics')).toBe(true);
    target.querySelector<HTMLElement>('.close')!.click();
    await settle();
    expect(lyricsPanel.open).toBe(false);
    expect(overlayOpen('lyrics')).toBe(false);
  });

  it('closes on Escape', async () => {
    show(LyricsPanel);
    toggleLyricsPanel(true);
    await settle();
    target.querySelector('aside')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(lyricsPanel.open).toBe(false);
  });

  it('opens from the transport strip button', async () => {
    show(TransportStrip);
    await settle();
    const button = target.querySelector<HTMLButtonElement>('button[aria-label="Lyrics"]')!;
    expect(button.getAttribute('aria-pressed')).toBe('false');
    button.click();
    flushSync();
    expect(lyricsPanel.open).toBe(true);
    expect(button.getAttribute('aria-pressed')).toBe('true');
    target.querySelector<HTMLButtonElement>('button[aria-label="Up next"]')!.click();
    flushSync();
    expect(lyricsPanel.open).toBe(false);
    expect(queuePanel.open).toBe(true);
  });

  it('opens with Ctrl+L and leaves Ctrl+Q for the queue', async () => {
    show(App);
    await settle();
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'l', ctrlKey: true, bubbles: true }));
    flushSync();
    expect(lyricsPanel.open).toBe(true);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'q', ctrlKey: true, bubbles: true }));
    flushSync();
    expect(queuePanel.open).toBe(true);
    expect(lyricsPanel.open).toBe(false);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'l', ctrlKey: true, bubbles: true }));
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'l', ctrlKey: true, bubbles: true }));
    flushSync();
    expect(lyricsPanel.open).toBe(false);
  });
});

describe('Now Playing', () => {
  it('switches to the lyrics on YouTube Music too', async () => {
    player.active = 'ytm';
    applyState(ytmPlaying);
    show(NowPlaying);
    await settle();
    expect(calls('lyrics_for_youtube')).toHaveLength(0);
    [...target.querySelectorAll<HTMLElement>('[role="radio"]')].find((b) => b.textContent?.trim() === 'Lyrics')!.click();
    await vi.waitFor(() => expect(calls('lyrics_for_youtube')).toHaveLength(1));
    await settle();
    expect(lines()).toHaveLength(4);
  });

  it('switches its side column between the queue and the lyrics and remembers it', async () => {
    play('a', 0);
    show(NowPlaying);
    await settle();
    expect(target.querySelector('#queue-heading')).not.toBeNull();
    expect(calls('lyrics_for_track')).toHaveLength(0);
    const pick = (label: string) =>
      [...target.querySelectorAll<HTMLElement>('[role="radio"]')].find((b) => b.textContent?.trim() === label)!;
    expect(pick('Up next').getAttribute('aria-checked')).toBe('true');

    pick('Lyrics').click();
    await settle();
    expect(nowPlayingSide.view).toBe('lyrics');
    expect(localStorage.getItem('prism.nowPlayingSide')).toBe('lyrics');
    expect(target.querySelector('#queue-heading')).toBeNull();
    expect(lines()).toHaveLength(4);

    pick('Up next').click();
    await settle();
    expect(target.querySelector('#queue-heading')).not.toBeNull();
    expect(target.querySelector('.lyrics')).toBeNull();
  });
});
