import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import source from '../../src-tauri/src/ytm/bridge.js?raw';
import fakePage from '../../src-tauri/src/ytm/testpage/index.html?raw';
import innertubeFixture from '../../src-tauri/src/ytm/testpage/innertube.json?raw';

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
/** Queue reports go here, with the time each was sent, so the state tests count as before. */
let queues: Report[] = [];
let queueTimes: number[] = [];
/** Lyrics answers go here. */
let lyricsReports: Report[] = [];
let session: { metadata: Meta | null; playbackState: string };
let win: FakeWindow;
let video: HTMLVideoElement | null;

function fakeWindow(): FakeWindow {
  const target = new EventTarget() as FakeWindow;
  target.top = target;
  target.__TAURI_INTERNALS__ = {
    invoke: (command: string, args: { report: Report }) => {
      expect(command).toBe('ytm_report');
      if (args.report.kind === 'queue') {
        queues.push(args.report);
        queueTimes.push(Date.now());
      } else if (args.report.kind === 'lyrics') {
        lyricsReports.push(args.report);
      } else {
        reports.push(args.report);
      }
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
  const player = document.createElement('ytmusic-player');
  player.appendChild(video);
  document.body.appendChild(player);
  if (!withBar) return;
  const bar = document.createElement('ytmusic-player-bar');
  button(bar, '#play-pause-button');
  button(bar, 'next-button');
  button(bar, 'previous-button');
  document.body.appendChild(bar);
}

function run(opts: { host?: string; hostname?: string; search?: string; dev?: boolean | 'unset'; framed?: boolean; w?: FakeWindow } = {}) {
  const w = opts.w ?? win;
  if (opts.framed) w.top = {};
  const loc = {
    host: opts.host ?? 'music.youtube.com',
    hostname: opts.hostname ?? (opts.host ?? 'music.youtube.com').split(':')[0],
    search: opts.search ?? '',
  };
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
  lyricsReports = [];
  queues = [];
  queueTimes = [];
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

it('hides_the_page_player_bar_but_still_uses_its_buttons', () => {
  play(true);
  run();
  const style = document.getElementById('prism-hide-bar');
  expect(style?.textContent).toContain('ytmusic-player-bar{visibility:hidden');
  // Once only, however often the heartbeat runs.
  vi.advanceTimersByTime(3000);
  expect(document.querySelectorAll('#prism-hide-bar')).toHaveLength(1);
  // The hidden bar still answers the bridge.
  expect(reports[0]).toMatchObject({ available: true, canNext: true });
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
    videoId: '',
  });
});

it('reports_the_video_id_from_the_player_else_the_url', () => {
  run({ search: '?v=urlVideo_01&list=RDAMVM' });
  expect(states()[0].videoId).toBe('urlVideo_01');
  const player = document.createElement('div');
  player.id = 'movie_player';
  Object.assign(player, { getVideoData: () => ({ video_id: 'playerVid01' }) });
  document.body.appendChild(player);
  vi.advanceTimersByTime(1000);
  expect(states().at(-1)!.videoId).toBe('playerVid01');
  // Anything that is not a video id is no video id.
  for (const bad of ['short', 'playerVid012', 'a/b/c/d/e/f', '<b>x</b>xx', 42, null]) {
    Object.assign(player, { getVideoData: () => ({ video_id: bad }) });
    vi.advanceTimersByTime(1000);
    expect(states().at(-1)!.videoId).toBe('urlVideo_01');
  }
  Object.assign(player, { getVideoData: () => { throw new Error('boom'); } });
  expect(() => vi.advanceTimersByTime(1000)).not.toThrow();
  expect(states().at(-1)!.videoId).toBe('urlVideo_01');
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
  const oldVideo = video!;
  oldVideo.replaceWith(fresh);
  video = fresh;
  vi.advanceTimersByTime(1000);
  const stale = reports.length;
  const old = oldVideo;
  (old as unknown as { setPaused: (p: boolean) => void }).setPaused(false);
  old.dispatchEvent(new Event('play'));
  expect(reports).toHaveLength(stale);
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

it('decoys_outside_the_player_are_ignored', () => {
  const decoyVideo = makeVideo();
  (decoyVideo as unknown as { setPaused: (p: boolean) => void }).setPaused(false);
  document.body.prepend(decoyVideo);
  const decoyNext = document.createElement('div');
  decoyNext.className = 'next-button';
  decoyNext.addEventListener('click', () => (clicks.decoy = (clicks.decoy ?? 0) + 1));
  document.body.prepend(decoyNext);
  run();
  expect(states()[0].playing).toBe(false);
  vi.advanceTimersByTime(3000);
  expect(reports).toHaveLength(1);
  command('next');
  expect(clicks.decoy).toBeUndefined();
  expect(clicks['next-button']).toBe(1);
});

it('no_bar_means_every_control_is_missing', () => {
  document.querySelector('ytmusic-player-bar')!.remove();
  const loose = document.createElement('div');
  loose.id = 'play-pause-button';
  loose.addEventListener('click', () => (clicks.loose = 1));
  document.body.appendChild(loose);
  run();
  expect(reports[0]).toMatchObject({ available: false, canNext: false, canPrev: false });
  command('toggle');
  expect(clicks.loose).toBeUndefined();
});

it('falls_back_to_movie_player_video', () => {
  document.body.innerHTML = '';
  buildPlayer();
  const player = document.querySelector('ytmusic-player')!;
  const movie = document.createElement('div');
  movie.id = 'movie_player';
  const v = makeVideo();
  (v as unknown as { setPaused: (p: boolean) => void }).setPaused(false);
  movie.appendChild(v);
  player.replaceWith(movie);
  run();
  expect(states()[0]).toMatchObject({ playing: true, available: true });
});

it('a_mutation_inside_the_bar_reports_right_away', async () => {
  run();
  const next = document.querySelector('.next-button')!;
  next.setAttribute('aria-disabled', 'true');
  await Promise.resolve();
  expect(states()).toHaveLength(2);
  expect(states()[1].canNext).toBe(false);
});

it('a_replaced_bar_is_re_observed', async () => {
  run();
  document.querySelector('ytmusic-player-bar')!.remove();
  const bar = document.createElement('ytmusic-player-bar');
  button(bar, '#play-pause-button');
  const next = button(bar, 'next-button');
  button(bar, 'previous-button');
  document.body.appendChild(bar);
  // The bar swap itself is picked up on the next tick; wait for it, then mutate the new bar.
  await vi.advanceTimersByTimeAsync(1000);
  const count = states().length;
  next.setAttribute('aria-disabled', 'true');
  await Promise.resolve();
  expect(states().length).toBe(count + 1);
  expect(states()[count].canNext).toBe(false);
});

it('works_wrapped_in_a_block', () => {
  const wrapped = `{ const ALLOW_DEV_HOST = false;\n${source}\n}`;
  new Function('window', 'location', 'navigator', wrapped)(win, { host: 'music.youtube.com', hostname: 'music.youtube.com' }, { mediaSession: session });
  expect(reports).toHaveLength(1);
  const dev = fakeWindow();
  new Function('window', 'location', 'navigator', `{ const ALLOW_DEV_HOST = true;\n${source}\n}`)(dev, { host: '127.0.0.1:9', hostname: '127.0.0.1' }, { mediaSession: session });
  expect(dev.__prismBridge).toBe(true);
});

it('the_fake_dev_page_satisfies_the_bridge', () => {
  const page = new DOMParser().parseFromString(fakePage, 'text/html');
  page.querySelectorAll('script').forEach((script) => script.remove());
  document.body.innerHTML = page.body.innerHTML;
  let nexts = 0;
  document.querySelector('ytmusic-player-bar .next-button')?.addEventListener('click', () => nexts++);
  session.metadata = { title: 'Demo Song 1', artist: 'Demo Artist 1', album: 'Demo Album 1' };
  const dev = { host: '127.0.0.1:4100', hostname: '127.0.0.1' };

  const wrapped = (flag: boolean) => `{ const ALLOW_DEV_HOST = ${flag};\n${source}\n}`;
  new Function('window', 'location', 'navigator', wrapped(false))(win, dev, { mediaSession: session });
  expect(reports).toHaveLength(0);

  const w = fakeWindow();
  win = w;
  new Function('window', 'location', 'navigator', wrapped(true))(w, dev, { mediaSession: session });
  expect(states().at(-1)).toMatchObject({
    available: true,
    playing: false,
    title: 'Demo Song 1',
    artist: 'Demo Artist 1',
    canNext: true,
    canPrev: true,
  });
  command('next');
  expect(nexts).toBe(1);

  const queue = queues.at(-1)!;
  expect(queue.hasQueue).toBe(true);
  expect(queue.items).toEqual([1, 2, 3].map((n) => ({ title: `Demo Song ${n}`, byline: `Demo Artist ${n}`, thumb: '', current: n === 1 })));
});

interface Entry {
  title: string;
  byline?: string;
  thumb?: string;
  /** How the item says it is the current one, if it is. */
  mark?: 'selected' | 'playing' | 'paused' | 'default';
  noPlayButton?: boolean;
}

/** Which element of which item got a click: `<index>:<tag>`. */
let queueClicks: string[] = [];

function queueItem(entry: Entry, index: number): HTMLElement {
  const item = document.createElement('ytmusic-player-queue-item');
  if (entry.mark === 'selected') item.setAttribute('selected', '');
  else if (entry.mark) item.setAttribute('play-button-state', entry.mark);
  const img = document.createElement('img');
  if (entry.thumb !== undefined) img.setAttribute('src', entry.thumb);
  item.appendChild(img);
  if (!entry.noPlayButton) item.appendChild(document.createElement('ytmusic-play-button-renderer'));
  const title = document.createElement('yt-formatted-string');
  title.className = 'song-title';
  title.textContent = entry.title;
  item.appendChild(title);
  const byline = document.createElement('yt-formatted-string');
  byline.className = 'byline';
  byline.textContent = entry.byline ?? '';
  item.appendChild(byline);
  item.addEventListener('click', (event) => queueClicks.push(`${index}:${(event.target as Element).tagName.toLowerCase()}`));
  return item;
}

function buildQueue(entries: Entry[]): HTMLElement {
  const queue = document.createElement('ytmusic-player-queue');
  entries.forEach((entry, i) => queue.appendChild(queueItem(entry, i)));
  document.body.appendChild(queue);
  return queue;
}

const THUMB = 'https://i.ytimg.com/vi/abc/default.jpg';

describe('the queue', () => {
  beforeEach(() => {
    queueClicks = [];
  });

  it('reads_the_queue_from_the_player_queue_only', () => {
    const decoy = queueItem({ title: 'Decoy', mark: 'selected' }, 99);
    document.body.prepend(decoy);
    buildQueue([
      { title: 'Eins', byline: 'Band A', thumb: THUMB },
      { title: '  Zwei  ', byline: 'Band B \n  •  Album', thumb: 'http://i.ytimg.com/vi/x/default.jpg', mark: 'selected' },
      { title: 'Drei', thumb: 'data:image/png;base64,AAAA' },
    ]);
    run();
    expect(queues).toHaveLength(1);
    expect(queues[0]).toEqual({
      v: 1,
      kind: 'queue',
      hasQueue: true,
      items: [
        { title: 'Eins', byline: 'Band A', thumb: THUMB, current: false },
        { title: 'Zwei', byline: 'Band B • Album', thumb: '', current: true },
        { title: 'Drei', byline: '', thumb: '', current: false },
      ],
    });
  });

  it('skips_the_hidden_twin_of_a_song', () => {
    const queue = buildQueue([{ title: 'Song', thumb: THUMB }]);
    const wrapper = document.createElement('ytmusic-playlist-panel-video-wrapper-renderer');
    const primary = document.createElement('div');
    primary.id = 'primary-renderer';
    primary.appendChild(queueItem({ title: 'Lied', byline: 'Band' }, 1));
    const twin = document.createElement('div');
    twin.id = 'counterpart-renderer';
    twin.appendChild(queueItem({ title: 'Lied (Video)', byline: 'Band' }, 2));
    wrapper.append(primary, twin);
    queue.appendChild(wrapper);
    run();
    expect((queues[0].items as Report[]).map((item) => item.title)).toEqual(['Song', 'Lied']);
  });

  it('takes_the_thumbnail_from_the_entry_data_before_the_lazy_image', () => {
    const queue = buildQueue([{ title: 'Off screen' }, { title: 'Loaded', thumb: THUMB }]);
    const [lazy, loaded] = queue.querySelectorAll('ytmusic-player-queue-item');
    Object.assign(lazy, {
      data: {
        thumbnail: {
          thumbnails: [
            { url: '//lh3.googleusercontent.com/a=w60', width: 60 },
            { url: 'https://lh3.googleusercontent.com/a=w226', width: 226 },
            { url: 'javascript:alert(1)', width: 999 },
          ],
        },
      },
    });
    Object.assign(loaded, { data: { thumbnail: { thumbnails: 'not a list' } } });
    run();
    expect((queues[0].items as Report[]).map((item) => item.thumb)).toEqual([
      'https://lh3.googleusercontent.com/a=w226',
      THUMB,
    ]);
  });

  it('knows_the_current_entry_by_either_mark', () => {
    buildQueue([
      { title: 'a', mark: 'selected' },
      { title: 'b', mark: 'playing' },
      { title: 'c', mark: 'paused' },
      { title: 'd', mark: 'default' },
      { title: 'e' },
    ]);
    run();
    expect((queues[0].items as Report[]).map((item) => item.current)).toEqual([true, true, true, false, false]);
  });

  it('caps_the_queue_and_its_strings', () => {
    const entries: Entry[] = Array.from({ length: 205 }, (_, i) => ({ title: `Lied ${i}` }));
    entries[0] = { title: 'T'.repeat(1000), byline: 'B'.repeat(300), thumb: 'https://i.ytimg.com/' + 'u'.repeat(2048) };
    entries[1] = { title: 'ok', thumb: 'https://i.ytimg.com/' + 'u'.repeat(2000) };
    buildQueue(entries);
    run();
    const items = queues[0].items as Report[];
    expect(items).toHaveLength(200);
    expect((items[0].title as string).length).toBe(256);
    expect((items[0].byline as string).length).toBe(256);
    expect(items[0].thumb).toBe('');
    expect((items[1].thumb as string).length).toBe(2020);
    expect(items[199].title).toBe('Lied 199');
  });

  it('leaves_out_the_tail_that_would_not_fit_the_report_cap', () => {
    // Quotes double when escaped, so these entries are about 3 KB each as JSON.
    const entries: Entry[] = Array.from({ length: 200 }, (_, i) => ({
      title: `${i}` + '"'.repeat(300),
      byline: '"'.repeat(300),
      thumb: 'https://i.ytimg.com/' + 'u'.repeat(2000),
    }));
    buildQueue(entries);
    run();
    const report = queues[0];
    const items = report.items as Report[];
    expect(items.length).toBeGreaterThan(50);
    expect(items.length).toBeLessThan(200);
    expect(new TextEncoder().encode(JSON.stringify(report)).length).toBeLessThanOrEqual(256 * 1024);
    // The head of the queue is kept, in order.
    expect((items[0].title as string).startsWith('0"')).toBe(true);
    expect((items.at(-1)!.title as string).startsWith(`${items.length - 1}"`)).toBe(true);
  });

  it('sends_the_queue_on_start_and_then_only_changes', async () => {
    const queue = buildQueue([{ title: 'Eins' }, { title: 'Zwei' }]);
    run();
    expect(queues).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(5000);
    expect(queues).toHaveLength(1);
    // A mutation that changes nothing the report carries.
    queue.firstElementChild!.setAttribute('aria-label', 'x');
    await vi.advanceTimersByTimeAsync(1000);
    expect(queues).toHaveLength(1);
    queue.appendChild(queueItem({ title: 'Drei' }, 2));
    await vi.advanceTimersByTimeAsync(500);
    expect(queues).toHaveLength(2);
    expect((queues[1].items as Report[]).map((item) => item.title)).toEqual(['Eins', 'Zwei', 'Drei']);
    // The state reports are not disturbed.
    expect(states()).toHaveLength(1);
  });

  it('queue_reports_are_throttled', async () => {
    const queue = buildQueue([{ title: 'Eins' }]);
    const title = queue.querySelector('.song-title')!;
    run();
    expect(queues).toHaveLength(1);
    // Right after the start report: the change waits until 500 ms have passed.
    for (let n = 0; n < 10; n++) {
      title.textContent = `Eins ${n}`;
      await Promise.resolve();
    }
    await vi.advanceTimersByTimeAsync(499);
    expect(queues).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(queues).toHaveLength(2);
    expect((queues[1].items as Report[])[0].title).toBe('Eins 9');
    // A change every 50 ms for 5 s: one report per 500 ms at most, and the last change arrives.
    for (let n = 0; n < 100; n++) {
      title.textContent = `Lauf ${n}`;
      await vi.advanceTimersByTimeAsync(50);
    }
    await vi.advanceTimersByTimeAsync(1000);
    expect(queues.length).toBeGreaterThan(5);
    expect(queues.length).toBeLessThanOrEqual(2 + 12);
    for (let i = 1; i < queueTimes.length; i++) expect(queueTimes[i] - queueTimes[i - 1]).toBeGreaterThanOrEqual(500);
    expect((queues.at(-1)!.items as Report[])[0].title).toBe('Lauf 99');
  });

  it('a_missing_queue_is_reported_and_followed_once_it_appears', async () => {
    run();
    expect(queues).toEqual([{ v: 1, kind: 'queue', items: [], hasQueue: false }]);
    await vi.advanceTimersByTimeAsync(3000);
    expect(queues).toHaveLength(1);

    const first = buildQueue([{ title: 'Eins', mark: 'selected' }]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(queues).toHaveLength(2);
    expect(queues[1]).toMatchObject({ hasQueue: true, items: [{ title: 'Eins', current: true }] });

    // YouTube Music replaces the element: the new one is observed, the old one no more.
    first.remove();
    const second = buildQueue([{ title: 'Neu' }]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(queues.at(-1)).toMatchObject({ hasQueue: true, items: [{ title: 'Neu' }] });
    const count = queues.length;
    first.querySelector('.song-title')!.textContent = 'Alt';
    await vi.advanceTimersByTimeAsync(500);
    expect(queues).toHaveLength(count);
    second.querySelector('.song-title')!.textContent = 'Neuer';
    await vi.advanceTimersByTimeAsync(500);
    expect(queues).toHaveLength(count + 1);
    expect((queues.at(-1)!.items as Report[])[0].title).toBe('Neuer');

    second.remove();
    await vi.advanceTimersByTimeAsync(1000);
    expect(queues.at(-1)).toEqual({ v: 1, kind: 'queue', items: [], hasQueue: false });
  });

  it('a_broken_queue_never_throws_into_the_page', async () => {
    const queue = buildQueue([{ title: 'Eins' }]);
    Object.defineProperty(queue, 'querySelectorAll', { value: () => { throw new Error('boom'); } });
    expect(() => run()).not.toThrow();
    expect(queues[0]).toEqual({ v: 1, kind: 'queue', items: [], hasQueue: true });
    win.__prismQueueIndex = 0;
    expect(() => command('queue')).not.toThrow();
    await vi.advanceTimersByTimeAsync(2000);
    expect(states()).toHaveLength(1);
  });

  it('queue_command_plays_the_entry_rust_chose', () => {
    const decoy = queueItem({ title: 'Decoy' }, 99);
    document.body.prepend(decoy);
    buildQueue([{ title: 'Eins' }, { title: 'Zwei' }, { title: 'Drei', noPlayButton: true }]);
    run();
    command('queue');
    expect(queueClicks).toEqual([]);
    win.__prismQueueIndex = 1;
    command('queue');
    expect(queueClicks).toEqual(['1:ytmusic-play-button-renderer']);
    win.__prismQueueIndex = 2;
    command('queue');
    expect(queueClicks.at(-1)).toBe('2:ytmusic-player-queue-item');
    win.__prismQueueIndex = 0;
    command('queue');
    expect(queueClicks.at(-1)).toBe('0:ytmusic-play-button-renderer');
    queueClicks = [];
    for (const bad of [3, 200, -1, 1.5, '1', Number.NaN, Infinity, null, { valueOf: () => 1 }]) {
      win.__prismQueueIndex = bad;
      command('queue');
    }
    expect(queueClicks).toEqual([]);
    // No other control is touched.
    expect(Object.keys(clicks)).toHaveLength(0);
  });

  it('queue_command_without_a_queue_does_nothing', () => {
    run();
    win.__prismQueueIndex = 0;
    expect(() => command('queue')).not.toThrow();
    expect(queueClicks).toEqual([]);
  });
});

/** The fake page's innertube answers, shared with the Rust fake server. */
const innertube = JSON.parse(innertubeFixture) as {
  next: Record<string, unknown>;
  browse: Record<string, unknown>;
};
const CONTEXT = { client: { clientName: 'WEB_REMIX', clientVersion: '1.20261022.01.00', hl: 'de' }, user: { lockedSafetyMode: false } };
const FIXTURE_TEXT = 'Erste Zeile vom Testlied\nZweite Zeile vom Testlied\n\nDer Refrain aus Testhausen';

interface Call {
  url: string;
  init: RequestInit;
  body: Record<string, unknown>;
}

describe('lyrics', () => {
  let calls: Call[];
  /** Replaces the answer for one endpoint; returns undefined to answer from the fixture. */
  let override: (endpoint: string, body: Record<string, unknown>, init: RequestInit) => Promise<Response> | undefined;

  function json(value: unknown, status = 200): Promise<Response> {
    return Promise.resolve(new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } }));
  }

  function fakeFetch(url: string, init: RequestInit): Promise<Response> {
    const body = JSON.parse(String(init.body)) as Record<string, unknown>;
    calls.push({ url, init, body });
    const endpoint = /^\/youtubei\/v1\/(\w+)\?prettyPrint=false$/.exec(url)?.[1] ?? '';
    const custom = override(endpoint, body, init);
    if (custom) return custom;
    if (endpoint === 'next' && typeof body.videoId === 'string' && body.videoId in innertube.next) {
      return json(innertube.next[body.videoId]);
    }
    if (endpoint === 'browse' && typeof body.browseId === 'string' && body.browseId in innertube.browse) {
      return json(innertube.browse[body.browseId]);
    }
    return json({ error: { code: 404 } }, 404);
  }

  function playVideo(id: unknown) {
    let player = document.getElementById('movie_player');
    if (!player) {
      player = document.createElement('div');
      player.id = 'movie_player';
      document.body.appendChild(player);
    }
    Object.assign(player, { getVideoData: () => ({ video_id: id }) });
  }

  async function ask(n: unknown) {
    win.__prismLyricsRequest = n;
    command('lyrics');
    await vi.advanceTimersByTimeAsync(0);
  }

  beforeEach(() => {
    calls = [];
    override = () => undefined;
    win.fetch = fakeFetch;
    win.ytcfg = { get: (key: string) => (key === 'INNERTUBE_CONTEXT' ? CONTEXT : undefined) };
    playVideo('demoSong001');
  });

  it('answers_a_request_with_the_lyrics_youtube_music_has', async () => {
    run();
    await ask(3);
    expect(lyricsReports).toEqual([
      { v: 1, kind: 'lyrics', request: 3, videoId: 'demoSong001', text: FIXTURE_TEXT, provider: 'Source: Testquelle', failed: false },
    ]);
    // Two requests, both on the page's own origin, as YouTube Music's own client sends them.
    expect(calls.map((call) => call.url)).toEqual(['/youtubei/v1/next?prettyPrint=false', '/youtubei/v1/browse?prettyPrint=false']);
    for (const call of calls) {
      expect(call.init.method).toBe('POST');
      expect(call.init.mode).toBe('same-origin');
      expect(call.init.credentials).toBe('omit');
      expect(call.body.context).toEqual(CONTEXT);
    }
    expect(calls[0].body).toMatchObject({ videoId: 'demoSong001', isAudioOnly: true });
    expect(calls[1].body).toMatchObject({ browseId: 'MPLYt_demoSong001' });
    expect(Object.keys(calls[1].body).sort()).toEqual(['browseId', 'context']);
    // The page's own context object is not touched.
    expect(CONTEXT).toEqual({ client: { clientName: 'WEB_REMIX', clientVersion: '1.20261022.01.00', hl: 'de' }, user: { lockedSafetyMode: false } });
  });

  it('no_lyrics_tab_or_an_unselectable_one_means_no_lyrics', async () => {
    run();
    playVideo('demoSong002');
    await ask(1);
    playVideo('demoSong003');
    await ask(2);
    expect(lyricsReports).toEqual([
      { v: 1, kind: 'lyrics', request: 1, videoId: 'demoSong002', text: null, provider: null, failed: false },
      { v: 1, kind: 'lyrics', request: 2, videoId: 'demoSong003', text: null, provider: null, failed: false },
    ]);
    // No browse request without a lyrics tab.
    expect(calls.map((call) => call.url)).toEqual(['/youtubei/v1/next?prettyPrint=false', '/youtubei/v1/next?prettyPrint=false']);
  });

  // Tabs Prism cannot read (renamed page types, moved endpoints) are a shape it does not
  // know: an error, never a miss for every song.
  it('tabs_without_any_known_page_type_fail', async () => {
    const tabs = (types: (string | null)[]) => ({
      contents: {
        singleColumnMusicWatchNextResultsRenderer: {
          tabbedRenderer: {
            watchNextTabbedResultsRenderer: {
              tabs: types.map((type) => ({
                tabRenderer: type === null ? { title: 'Up next' } : { title: 'x', endpoint: { browseEndpoint: { browseId: 'MPLYt_x', browseEndpointContextSupportedConfigs: { browseEndpointContextMusicConfig: { pageType: type } } } } },
              })),
            },
          },
        },
      },
    });
    run();
    for (const [i, types] of [[null], [null, 'MUSIC_PAGE_TYPE_LYRICS_V2', 'MUSIC_PAGE_TYPE_RELATED_V2'], []].entries()) {
      override = (endpoint) => (endpoint === 'next' ? json(tabs(types)) : undefined);
      await ask(i + 1);
    }
    // A known page type without a lyrics tab is a song without lyrics.
    override = (endpoint) => (endpoint === 'next' ? json(tabs([null, 'MUSIC_PAGE_TYPE_TRACK_RELATED'])) : undefined);
    await ask(9);
    expect(lyricsReports.map((r) => [r.request, r.failed])).toEqual([
      [1, true],
      [2, true],
      [3, true],
      [9, false],
    ]);
    expect(lyricsReports.every((r) => r.text === null)).toBe(true);
    expect(calls.every((call) => call.url.startsWith('/youtubei/v1/next'))).toBe(true);
  });

  // The tab promised lyrics, so a page without them is a shape Prism does not know: an
  // error, never a miss that would be kept for days.
  it('a_lyrics_page_without_a_description_shelf_fails', async () => {
    override = (endpoint) => (endpoint === 'browse' ? json({ contents: { sectionListRenderer: { contents: [{ messageRenderer: {} }] } } }) : undefined);
    run();
    await ask(1);
    expect(lyricsReports).toEqual([{ v: 1, kind: 'lyrics', request: 1, videoId: 'demoSong001', text: null, provider: null, failed: true }]);
  });

  it('joins_every_run_and_skips_odd_ones', async () => {
    override = (endpoint) =>
      endpoint === 'browse'
        ? json({
            contents: {
              sectionListRenderer: {
                contents: [
                  {
                    musicDescriptionShelfRenderer: {
                      description: { runs: [{ text: 'Eins\n' }, { text: 42 }, null, { text: 'Zwei' }] },
                      footer: { runs: [{ text: 'Source: ' }, { text: 'Testquelle' }] },
                    },
                  },
                ],
              },
            },
          })
        : undefined;
    run();
    await ask(1);
    expect(lyricsReports[0]).toMatchObject({ text: 'Eins\nZwei', provider: 'Source: Testquelle', failed: false });
  });

  it('a_failed_request_is_reported_as_failed_not_as_no_lyrics', async () => {
    const failures: (() => Promise<Response>)[] = [
      () => Promise.reject(new TypeError('network')),
      () => json({ error: {} }, 500),
      () => Promise.resolve(new Response('<html>nope</html>', { status: 200 })),
      () => json('a string'),
      () => json({ contents: { singleColumnMusicWatchNextResultsRenderer: {} } }),
    ];
    run();
    for (const [i, failure] of failures.entries()) {
      override = (endpoint) => (endpoint === 'next' ? failure() : undefined);
      await ask(i + 1);
    }
    // The browse step fails too.
    override = (endpoint) => (endpoint === 'browse' ? json({}, 403) : undefined);
    await ask(10);
    expect(lyricsReports.map((r) => r.request)).toEqual([1, 2, 3, 4, 5, 10]);
    for (const report of lyricsReports) {
      expect(report).toMatchObject({ kind: 'lyrics', videoId: 'demoSong001', text: null, provider: null, failed: true });
    }
  });

  it('a_browse_id_that_is_not_one_fails_without_a_request', async () => {
    for (const bad of ['MPLYt/../../x', 'MPLYt_x?y=1', '', 42, 'x'.repeat(200)]) {
      override = (endpoint) => {
        if (endpoint !== 'next') return undefined;
        const answer = JSON.parse(JSON.stringify(innertube.next.demoSong001));
        answer.contents.singleColumnMusicWatchNextResultsRenderer.tabbedRenderer.watchNextTabbedResultsRenderer.tabs[1].tabRenderer.endpoint.browseEndpoint.browseId = bad;
        return json(answer);
      };
      calls = [];
      lyricsReports = [];
      win = fakeWindow();
      win.fetch = fakeFetch;
      win.ytcfg = { get: () => CONTEXT };
      run();
      await ask(1);
      expect(calls).toHaveLength(1);
      expect(lyricsReports).toEqual([{ v: 1, kind: 'lyrics', request: 1, videoId: 'demoSong001', text: null, provider: null, failed: true }]);
    }
  });

  it('takes_the_video_id_from_the_url_without_a_player', async () => {
    document.getElementById('movie_player')!.remove();
    run({ search: '?v=demoSong001&list=RDAMVMdemoSong001' });
    await ask(1);
    expect(lyricsReports[0]).toMatchObject({ videoId: 'demoSong001', text: FIXTURE_TEXT });
  });

  it('ignores_bad_request_numbers_and_missing_video_ids', async () => {
    run();
    for (const bad of [0, -1, 1.5, '3', Number.NaN, Infinity, 2 ** 32, null, undefined, { valueOf: () => 1 }]) {
      await ask(bad);
    }
    playVideo('not an id');
    await ask(1);
    document.getElementById('movie_player')!.remove();
    await ask(2);
    expect(calls).toHaveLength(0);
    expect(lyricsReports).toHaveLength(0);
    // The largest request number still counts.
    playVideo('demoSong001');
    await ask(2 ** 32 - 1);
    expect(lyricsReports[0]).toMatchObject({ request: 2 ** 32 - 1 });
  });

  it('works_without_the_page_config', async () => {
    delete win.ytcfg;
    run();
    await ask(1);
    expect(lyricsReports[0]).toMatchObject({ text: FIXTURE_TEXT, failed: false });
    const client = (calls[0].body.context as { client: Record<string, string> }).client;
    expect(client.clientName).toBe('WEB_REMIX');
    expect(client.clientVersion).toMatch(/^1\.\d{8}\.01\.00$/);
    // A config that throws or hands back something odd counts as missing.
    win.ytcfg = { get: () => { throw new Error('boom'); } };
    await ask(2);
    win.ytcfg = { get: () => 'context' };
    await ask(3);
    expect(lyricsReports.map((r) => r.failed)).toEqual([false, false, false]);
  });

  it('long_lyrics_are_cut_to_fit_the_report_cap', async () => {
    // Quotes double when escaped, so the cut must count the report as JSON.
    const long = ('Zeile "aus" Testhausen\n').repeat(5000);
    override = (endpoint) =>
      endpoint === 'browse'
        ? json({ contents: { sectionListRenderer: { contents: [{ musicDescriptionShelfRenderer: { description: { runs: [{ text: long }] }, footer: { runs: [{ text: 'P'.repeat(1000) }] } } }] } } })
        : undefined;
    run();
    await ask(1);
    const report = lyricsReports[0];
    expect(new TextEncoder().encode(JSON.stringify(report)).length).toBeLessThanOrEqual(60 * 1024);
    expect((report.text as string).length).toBeGreaterThan(20 * 1024);
    expect(long.startsWith(report.text as string)).toBe(true);
    expect((report.provider as string).length).toBe(256);
  });

  it('a_cut_never_splits_a_surrogate_pair', async () => {
    const astral = 'a'.repeat(255) + '\u{1F3B5}tail';
    override = (endpoint) =>
      endpoint === 'browse'
        ? json({ contents: { sectionListRenderer: { contents: [{ musicDescriptionShelfRenderer: { description: { runs: [{ text: 'Eins' }] }, footer: { runs: [{ text: astral }] } } }] } } })
        : undefined;
    session.metadata!.title = astral;
    run();
    await ask(1);
    expect(lyricsReports[0].provider).toBe('a'.repeat(255));
    expect(states()[0].title).toBe('a'.repeat(255));
    // A pair that fits whole is kept.
    expect(lyricsReports[0].text).toBe('Eins');
  });

  it('gives_up_after_its_time_limit', async () => {
    override = (_endpoint, _body, init) =>
      new Promise<Response>((_, reject) => {
        init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
      });
    run();
    await ask(1);
    expect(lyricsReports).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(6999);
    expect(lyricsReports).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(lyricsReports).toEqual([{ v: 1, kind: 'lyrics', request: 1, videoId: 'demoSong001', text: null, provider: null, failed: true }]);
  });

  it('a_lyrics_request_never_throws_into_the_page', async () => {
    win.fetch = () => {
      throw new Error('boom');
    };
    run();
    await expect(ask(1)).resolves.toBeUndefined();
    expect(lyricsReports[0]).toMatchObject({ failed: true });
    delete win.fetch;
    await ask(2);
    expect(lyricsReports[1]).toMatchObject({ request: 2, failed: true });
    // The rest of the bridge goes on.
    command('next');
    expect(clicks['next-button']).toBe(1);
  });
});
