import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import source from '../../src-tauri/src/ytm/bridge.js?raw';

type Report = Record<string, unknown>;
type FakeWindow = EventTarget & Record<string, unknown>;

interface Meta {
  title?: string;
  artist?: string;
  album?: string;
  artwork?: { src: string; sizes?: string }[];
}

const clicks: Record<string, number> = {};
let reports: Report[] = [];
let session: { metadata: Meta | null; playbackState: string };
let win: FakeWindow;
let video: HTMLVideoElement | null;

function fakeWindow(): FakeWindow {
  const target = new EventTarget() as FakeWindow;
  target.top = target;
  target.__TAURI_INTERNALS__ = {
    invoke: (command: string, args: { report: Report }) => {
      expect(command).toBe('ytm_report');
      reports.push(args.report);
      return Promise.resolve();
    },
  };
  return target;
}

function makeVideo(): HTMLVideoElement {
  const el = document.createElement('video');
  let paused = true;
  let currentTime = 0;
  Object.defineProperty(el, 'paused', { get: () => paused, configurable: true });
  Object.defineProperty(el, 'currentTime', { get: () => currentTime, set: (v: number) => (currentTime = v), configurable: true });
  Object.defineProperty(el, 'duration', { get: () => 200, configurable: true });
  (el as unknown as { setPaused: (p: boolean) => void }).setPaused = (p) => (paused = p);
  return el;
}

function button(el: HTMLElement, cls: string): HTMLElement {
  const b = document.createElement('yt-icon-button');
  b.className = cls;
  if (cls.startsWith('#')) {
    b.className = '';
    b.id = cls.slice(1);
  }
  b.addEventListener('click', () => (clicks[cls] = (clicks[cls] ?? 0) + 1));
  el.appendChild(b);
  return b;
}

function buildPlayer(withBar = true) {
  document.body.innerHTML = '';
  video = makeVideo();
  document.body.appendChild(video);
  if (!withBar) return;
  const bar = document.createElement('ytmusic-player-bar');
  button(bar, '#play-pause-button');
  button(bar, 'next-button');
  button(bar, 'previous-button');
  document.body.appendChild(bar);
}

function run(opts: { host?: string; hostname?: string; dev?: boolean | 'unset'; framed?: boolean; w?: FakeWindow } = {}) {
  const w = opts.w ?? win;
  if (opts.framed) w.top = {};
  const loc = { host: opts.host ?? 'music.youtube.com', hostname: opts.hostname ?? (opts.host ?? 'music.youtube.com').split(':')[0] };
  const nav = { mediaSession: session };
  const prefix = opts.dev === undefined || opts.dev === 'unset' ? '' : `const ALLOW_DEV_HOST = ${opts.dev};\n`;
  new Function('window', 'location', 'navigator', prefix + source)(w, loc, nav);
}

function play(on: boolean) {
  (video as unknown as { setPaused: (p: boolean) => void }).setPaused(!on);
}

function command(action: string) {
  win.dispatchEvent(new CustomEvent('prism-ytm', { detail: action }));
}

function states(): Report[] {
  return reports.filter((r) => r.kind === 'state');
}

beforeEach(() => {
  vi.useFakeTimers();
  reports = [];
  for (const k of Object.keys(clicks)) delete clicks[k];
  session = {
    metadata: {
      title: 'Testlied',
      artist: 'Test Band',
      album: 'Testalbum',
      artwork: [
        { src: 'https://lh3.googleusercontent.com/a=s60', sizes: '60x60' },
        { src: 'https://lh3.googleusercontent.com/a=s544', sizes: '544x544' },
        { src: 'https://lh3.googleusercontent.com/a=s226', sizes: '226x226' },
      ],
    },
    playbackState: 'paused',
  };
  win = fakeWindow();
  buildPlayer();
});

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = '';
});

it('reports_metadata_and_play_state', () => {
  play(true);
  (video as HTMLVideoElement).currentTime = 12.34;
  run();
  expect(reports).toHaveLength(1);
  expect(reports[0]).toEqual({
    v: 1,
    kind: 'state',
    playing: true,
    title: 'Testlied',
    artist: 'Test Band',
    album: 'Testalbum',
    artwork: 'https://lh3.googleusercontent.com/a=s544',
    positionMs: 12340,
    durationMs: 200000,
    canNext: true,
    canPrev: true,
    available: true,
  });
});

it('picks_the_largest_https_artwork', () => {
  session.metadata!.artwork = [
    { src: 'http://example.test/huge', sizes: '1000x1000' },
    { src: 'https://lh3.googleusercontent.com/small', sizes: '60x60' },
    { src: 'https://lh3.googleusercontent.com/mid', sizes: '226x226' },
  ];
  run();
  expect(states()[0].artwork).toBe('https://lh3.googleusercontent.com/mid');
});

it('heartbeat_only_while_playing', () => {
  run();
  vi.advanceTimersByTime(5000);
  expect(reports).toHaveLength(1);
  play(true);
  vi.advanceTimersByTime(3000);
  // One report for the change to playing, then one per second.
  expect(reports).toHaveLength(1 + 3);
  play(false);
  vi.advanceTimersByTime(1000);
  const count = reports.length;
  vi.advanceTimersByTime(5000);
  expect(reports).toHaveLength(count);
});

it('unchanged_state_is_not_resent', () => {
  run();
  vi.advanceTimersByTime(4000);
  video!.dispatchEvent(new Event('durationchange'));
  expect(reports).toHaveLength(1);
  session.metadata!.title = 'Anderes Lied';
  vi.advanceTimersByTime(1000);
  expect(reports).toHaveLength(2);
  expect(states()[1].title).toBe('Anderes Lied');
});

it('video_events_report_right_away', () => {
  run();
  play(true);
  video!.dispatchEvent(new Event('play'));
  expect(reports).toHaveLength(2);
  expect(states()[1].playing).toBe(true);
  play(false);
  video!.dispatchEvent(new Event('ended'));
  expect(reports.some((r) => r.kind === 'ended' && r.v === 1)).toBe(true);
  expect(reports[reports.length - 1].playing).toBe(false);
});

it('toggle_clicks_play_pause', () => {
  run();
  command('toggle');
  expect(clicks['#play-pause-button']).toBe(1);
  command('play');
  expect(clicks['#play-pause-button']).toBe(2);
  command('pause');
  expect(clicks['#play-pause-button']).toBe(2);
  play(true);
  command('play');
  expect(clicks['#play-pause-button']).toBe(2);
  command('pause');
  expect(clicks['#play-pause-button']).toBe(3);
});

it('next_and_prev_click_their_buttons', () => {
  run();
  command('next');
  command('next');
  command('prev');
  expect(clicks['next-button']).toBe(2);
  expect(clicks['previous-button']).toBe(1);
});

it('seek_and_volume_read_the_numbers_rust_set', () => {
  run();
  win.__prismSeekMs = 42500;
  command('seek');
  expect(video!.currentTime).toBe(42.5);
  win.__prismVolume = 0.25;
  command('volume');
  expect(video!.volume).toBe(0.25);
  win.__prismVolume = 7;
  command('volume');
  expect(video!.volume).toBe(1);
});

it('ignores_unknown_actions_and_bad_numbers', () => {
  run();
  const before = reports.length;
  win.__prismSeekMs = Number.NaN;
  command('seek');
  win.__prismSeekMs = Infinity;
  command('seek');
  win.__prismSeekMs = '5000';
  command('seek');
  win.__prismVolume = Number.NaN;
  command('volume');
  command('format-disk');
  command('');
  win.dispatchEvent(new CustomEvent('prism-ytm', { detail: { toString: () => 'toggle' } }));
  win.dispatchEvent(new CustomEvent('prism-ytm'));
  expect(video!.currentTime).toBe(0);
  expect(Object.keys(clicks)).toHaveLength(0);
  expect(reports).toHaveLength(before);
});

it('does_nothing_on_other_hosts', () => {
  run({ host: 'www.example.test' });
  run({ host: '127.0.0.1:1234', hostname: '127.0.0.1' });
  run({ host: '127.0.0.1:1234', hostname: '127.0.0.1', dev: false });
  vi.advanceTimersByTime(3000);
  expect(reports).toHaveLength(0);
  expect(win.__prismBridge).toBeUndefined();
});

it('accepts_loopback_only_with_the_dev_flag', () => {
  run({ host: '127.0.0.1:1234', hostname: '127.0.0.1', dev: true });
  expect(reports).toHaveLength(1);
  const other = fakeWindow();
  run({ host: 'localhost:1234', hostname: 'localhost', dev: true, w: other });
  expect(other.__prismBridge).toBeUndefined();
});

it('does_nothing_in_frames', () => {
  run({ framed: true });
  vi.advanceTimersByTime(3000);
  expect(reports).toHaveLength(0);
  expect(win.__prismBridge).toBeUndefined();
});

it('runs_once', () => {
  run();
  run();
  vi.advanceTimersByTime(2000);
  expect(reports).toHaveLength(1);
  command('toggle');
  expect(clicks['#play-pause-button']).toBe(1);
});

it('bridge_survives_a_dom_without_the_player_bar', () => {
  buildPlayer(false);
  session.metadata = null;
  expect(() => run()).not.toThrow();
  expect(() => vi.advanceTimersByTime(3000)).not.toThrow();
  expect(reports).toHaveLength(1);
  expect(reports[0]).toMatchObject({ available: false, playing: false, canNext: false, canPrev: false, title: '', artwork: '' });
  expect(() => ['toggle', 'play', 'pause', 'next', 'prev', 'seek', 'volume'].forEach(command)).not.toThrow();
  document.body.innerHTML = '';
  expect(() => vi.advanceTimersByTime(3000)).not.toThrow();
  expect(reports[reports.length - 1]).toMatchObject({ available: false, playing: false });
});

it('missing_mediasession_is_tolerated', () => {
  const w = fakeWindow();
  const nav = {};
  new Function('window', 'location', 'navigator', source)(w, { host: 'music.youtube.com', hostname: 'music.youtube.com' }, nav);
  expect(reports[0]).toMatchObject({ title: '', available: true });
});

it('video_swap_is_followed', () => {
  run();
  const fresh = makeVideo();
  video!.replaceWith(fresh);
  video = fresh;
  vi.advanceTimersByTime(1000);
  const before = reports.length;
  play(true);
  fresh.dispatchEvent(new Event('play'));
  expect(reports.length).toBe(before + 1);
  expect(reports[reports.length - 1].playing).toBe(true);
  command('seek');
  win.__prismSeekMs = 9000;
  command('seek');
  expect(fresh.currentTime).toBe(9);
});

it('strings_are_capped', () => {
  session.metadata = {
    title: 'T'.repeat(1000),
    artist: 'A'.repeat(300),
    album: 'B'.repeat(257),
    artwork: [
      { src: 'https://x.test/' + 'u'.repeat(2048), sizes: '900x900' },
      { src: 'https://x.test/ok', sizes: '10x10' },
    ],
  };
  run();
  const state = states()[0];
  expect((state.title as string).length).toBe(256);
  expect((state.artist as string).length).toBe(256);
  expect((state.album as string).length).toBe(256);
  expect(state.artwork).toBe('https://x.test/ok');
});

it('sends_nothing_without_tauri_internals', () => {
  delete win.__TAURI_INTERNALS__;
  expect(() => run()).not.toThrow();
  expect(() => vi.advanceTimersByTime(2000)).not.toThrow();
  expect(reports).toHaveLength(0);
});
