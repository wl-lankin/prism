import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Station, Track } from '../types';

const ID = 'radio:11111111-2222-3333-4444-555555555555';

const station = (over: Partial<Station> = {}): Station => ({
  id: ID,
  name: 'Radio Testhausen',
  url: 'http://127.0.0.1:1/stream',
  urlResolved: null,
  homepage: 'https://radio.example/',
  favicon: null,
  coverId: null,
  countryCode: 'DE',
  region: 'Testland',
  language: null,
  tags: [],
  codec: 'MP3',
  bitrateKbps: 128,
  hls: false,
  lastCheckOk: true,
  distanceKm: null,
  ...over,
});

const live = (over: Partial<Track> = {}): Track => ({
  id: ID,
  source: 'radio',
  title: 'Radio Testhausen',
  artist: 'Testland, DE',
  albumArtist: '',
  album: '',
  albumId: '',
  trackNo: 0,
  discNo: 0,
  durationMs: 0,
  year: null,
  genre: null,
  format: 'MP3',
  sampleRate: null,
  bitDepth: null,
  bitrateKbps: 128,
  coverId: null,
  path: null,
  replayGainTrackDb: null,
  replayGainAlbumDb: null,
  replayGainTrackPeak: null,
  replayGainAlbumPeak: null,
  ...over,
});

const invoke = vi.fn(async (cmd: string, _args?: Record<string, unknown>): Promise<unknown> => {
  switch (cmd) {
    case 'radio_saved':
      return { favorites: [], recent: [station()] };
    case 'radio_favorite':
      return true;
    case 'player_queue_range':
      return [{ index: 0, track: live() }];
    default:
      return undefined;
  }
});
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
const openUrl = vi.fn(async (_url: string) => {});
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: (url: string) => openUrl(url) }));

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= function () {};

const { default: TransportStrip } = await import('./TransportStrip.svelte');
const { default: TrackMenu } = await import('./TrackMenu.svelte');
const { player, showPlayerError } = await import('../stores/player.svelte');
const { radio } = await import('../stores/radio.svelte');
const { favorites } = await import('../stores/favorites.svelte');
const { notice } = await import('../stores/notice.svelte');
const { setLang } = await import('../i18n');

let target: HTMLElement;
beforeEach(() => {
  setLang('en');
  invoke.mockClear();
  openUrl.mockClear();
  notice.text = null;
  radio.nowPlaying = null;
  radio.saved = { favorites: [], recent: [] };
  favorites.stations.clear();
  Object.assign(player, {
    status: 'playing',
    track: live(),
    positionMs: 65_000,
    durationMs: 0,
    queueIndex: 0,
    queueLength: 1,
    queueVersion: 1,
  });
  target = document.createElement('div');
  document.body.append(target);
});
afterEach(() => {
  target.remove();
  vi.useRealTimers();
});

async function settle() {
  for (let i = 0; i < 6; i++) {
    await tick();
    await Promise.resolve();
  }
  flushSync();
}

describe('transport strip in live mode', () => {
  it('live_track_shows_pill_not_seekbar', async () => {
    const app = mount(TransportStrip, { target });
    await settle();
    expect(target.querySelector('.pill')?.textContent?.trim()).toBe('LIVE');
    expect(target.querySelector('[role="slider"][aria-label="Position"]')).toBeNull();
    // The time heard stays; there is no duration after it.
    expect(target.querySelector('.time.elapsed')?.textContent).toBe('1:05');
    expect(target.querySelectorAll('.time')[1].textContent?.trim()).toBe('');
    expect(target.querySelector('.quality')?.textContent?.trim()).toBe('MP3 128');
    unmount(app);
  });

  it('a normal track keeps its seek bar', async () => {
    Object.assign(player, { track: live({ id: 'local:t1', source: 'local', durationMs: 180_000, path: 'C:/a.flac' }), durationMs: 180_000 });
    const app = mount(TransportStrip, { target });
    await settle();
    expect(target.querySelector('.pill')).toBeNull();
    expect(target.querySelector('[role="slider"][aria-label="Position"]')).not.toBeNull();
    // A track keeps its cover; the station tile is only for stations.
    expect(target.querySelector('.now div.cover')).not.toBeNull();
    expect(target.querySelector('.tile')).toBeNull();
    unmount(app);
  });

  it('a_station_shows_the_station_tile', async () => {
    const app = mount(TransportStrip, { target });
    await settle();
    expect(target.querySelector('.now .tile .initials')?.textContent).toBe('RT');
    expect(target.querySelector('.now div.cover')).toBeNull();
    unmount(app);

    // With a logo the tile loads it from the cover cache.
    Object.assign(player, { track: live({ coverId: 'rb-0123456789abcdef0123' }) });
    const withLogo = mount(TransportStrip, { target });
    await settle();
    expect(target.querySelector('.now .tile img')?.getAttribute('src')).toContain('/cover/rb-0123456789abcdef0123/96');
    unmount(withLogo);
  });

  it('live_disables_next_and_prev', async () => {
    const app = mount(TransportStrip, { target });
    await settle();
    for (const label of ['Previous', 'Next']) {
      const el = target.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
      expect(el.disabled).toBe(true);
      expect(el.title).toBe('Not available while a station plays');
    }
    unmount(app);
  });

  it('stream_title_is_the_title_line', async () => {
    radio.nowPlaying = { stationId: ID, title: 'Glass Tides - Lumen Field' };
    const app = mount(TransportStrip, { target });
    await settle();
    expect(target.querySelector('.title')?.textContent?.trim()).toBe('Glass Tides - Lumen Field');
    expect(target.querySelector('.sub')?.textContent?.trim()).toBe('Radio Testhausen');

    // No title: the station name over its region line.
    radio.nowPlaying = { stationId: ID, title: null };
    await settle();
    expect(target.querySelector('.title')?.textContent?.trim()).toBe('Radio Testhausen');
    expect(target.querySelector('.sub')?.textContent?.trim()).toBe('Testland, DE');
    unmount(app);
  });

  it('title_of_another_station_is_ignored', async () => {
    radio.nowPlaying = { stationId: 'radio:99999999-2222-3333-4444-555555555555', title: 'Someone else' };
    const app = mount(TransportStrip, { target });
    await settle();
    expect(target.querySelector('.title')?.textContent?.trim()).toBe('Radio Testhausen');
    expect(target.textContent).not.toContain('Someone else');
    unmount(app);
  });

  it('its heart saves the station through the radio store', async () => {
    const app = mount(TransportStrip, { target });
    await settle();
    expect(invoke).toHaveBeenCalledWith('radio_saved', undefined);
    const heart = target.querySelector<HTMLButtonElement>('.heart')!;
    heart.click();
    await settle();
    expect(invoke).toHaveBeenCalledWith('radio_favorite', expect.objectContaining({ on: true }));
    expect(invoke).not.toHaveBeenCalledWith('favorites_set', expect.anything());
    unmount(app);
  });
});

describe('station tile on the other surfaces', () => {
  async function frames() {
    for (let i = 0; i < 4; i++) {
      await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
      await settle();
    }
  }

  it('now_playing_shows_the_station_tile', async () => {
    const { default: NowPlaying } = await import('./NowPlaying.svelte');
    const app = mount(NowPlaying, { target });
    await frames();
    expect(target.querySelector('.art .tile.fluid .initials')?.textContent).toBe('RT');
    expect(target.querySelector('.art .cover')).toBeNull();
    await vi.waitFor(async () => {
      await frames();
      expect(target.querySelector('.qrow .tile .initials')?.textContent).toBe('RT');
    });
    unmount(app);
  });

  it('queue_panel_shows_the_station_tile', async () => {
    const { default: QueuePanel } = await import('./QueuePanel.svelte');
    const { queuePanel } = await import('../stores/queue.svelte');
    queuePanel.open = true;
    const app = mount(QueuePanel, { target });
    await vi.waitFor(async () => {
      await frames();
      expect(target.querySelector('.row .tile .initials')?.textContent).toBe('RT');
    });
    expect(target.querySelector('.row .cover')).toBeNull();
    unmount(app);
    queuePanel.open = false;
  });
});

describe('station menu', () => {
  it('station_menu_has_no_queue_items', async () => {
    const onclose = vi.fn();
    const app = mount(TrackMenu, { target, props: { trackIds: [ID], x: 10, y: 10, station: station(), onclose } });
    flushSync();
    const labels = [...target.querySelectorAll('[role="menuitem"] .label')].map((el) => el.textContent?.trim());
    expect(labels).toEqual(['Save station', 'Open homepage']);
    expect(target.querySelector('.has-sub')).toBeNull();
    unmount(app);
  });

  it('homepage_opens_through_the_backend', async () => {
    const onclose = vi.fn();
    const app = mount(TrackMenu, { target, props: { trackIds: [ID], x: 10, y: 10, station: station(), onclose } });
    flushSync();
    const item = [...target.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(
      (el) => el.textContent?.trim() === 'Open homepage',
    )!;
    item.click();
    await settle();
    expect(onclose).toHaveBeenCalled();
    expect(invoke).toHaveBeenCalledWith('radio_open_homepage', { stationId: ID });
    // The webview never opens a station's link itself.
    expect(openUrl).not.toHaveBeenCalled();
    unmount(app);
  });

  it('a station without a homepage has the favorite item only', () => {
    const app = mount(TrackMenu, {
      target,
      props: { trackIds: [ID], x: 10, y: 10, station: station({ homepage: 'javascript:void(0)' }), onclose: vi.fn() },
    });
    flushSync();
    expect(target.querySelectorAll('[role="menuitem"]').length).toBe(1);
    unmount(app);
  });
});

describe('player errors', () => {
  it('radio_offline_notice_names_the_station', () => {
    radio.saved.recent = [station()];
    showPlayerError({ trackId: ID, message: 'The station is not reachable', kind: 'offline', source: 'radio' });
    expect(notice.text).toBe('Radio Testhausen is not reachable. Press Play to try again.');
    // A Navidrome hold keeps its own text.
    notice.text = null;
    showPlayerError({ trackId: 'navidrome:s1', message: 'x', kind: 'offline', source: 'navidrome' });
    expect(notice.text).toBe('Navidrome is not reachable. Press Play once it is back.');
    // An error without `source` is a Navidrome one.
    notice.text = null;
    showPlayerError({ trackId: 'navidrome:s1', message: 'x', kind: 'offline' } as never);
    expect(notice.text).toBe('Navidrome is not reachable. Press Play once it is back.');
    setLang('de');
    notice.text = null;
    showPlayerError({ trackId: ID, message: 'x', kind: 'offline', source: 'radio' });
    expect(notice.text).toBe('Radio Testhausen ist nicht erreichbar. Mit Abspielen erneut versuchen.');
  });

  it('track_error_shows_a_notice', () => {
    showPlayerError({ trackId: 'local:t1', message: 'decode failed', kind: 'track', source: 'local' });
    expect(notice.text).toBe('This track cannot be played.');

    // A station that fails to open says so with its name.
    notice.text = null;
    showPlayerError({ trackId: ID, message: 'This station is not sending audio', kind: 'track', source: 'radio' });
    expect(notice.text).toBe('Radio Testhausen cannot be played.');
  });

  it('names the track that is shown', () => {
    Object.assign(player, { track: live({ id: 'local:t9', source: 'local', title: 'Glass Tides', durationMs: 1000 }) });
    showPlayerError({ trackId: 'local:t9', message: 'decode failed', kind: 'track', source: 'local' });
    expect(notice.text).toBe('Glass Tides cannot be played.');
  });
});
