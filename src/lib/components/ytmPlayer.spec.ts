import { flushSync, mount, tick, unmount, type Component } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { YtmState } from '../types';

const invoke = vi.fn(async (cmd: string, _args?: Record<string, unknown>): Promise<unknown> => {
  if (cmd === 'player_state') {
    return { status: 'idle', track: null, positionMs: 0, durationMs: 0, volume: 0.8, queueIndex: 0, queueLength: 0, queueVersion: 0 };
  }
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
type DragHandler = (event: { payload: { type: string; paths?: string[] } }) => void;
let dragHandler: DragHandler | undefined;
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isMaximized: async () => false,
    onResized: async () => () => {},
    onDragDropEvent: async (handler: DragHandler) => {
      dragHandler = handler;
      return () => {};
    },
  }),
}));

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= function () {};

const { default: App } = await import('../../App.svelte');
const { default: TransportStrip } = await import('./TransportStrip.svelte');
const { default: NowPlaying } = await import('./NowPlaying.svelte');
const { default: QueuePanel } = await import('./QueuePanel.svelte');
const { default: FlowRing } = await import('./FlowRing.svelte');
const { default: SearchPalette } = await import('./SearchPalette.svelte');
const { default: TrackMenu } = await import('./TrackMenu.svelte');
const { default: StationMenu } = await import('./StationMenu.svelte');
const { default: FlowMenu } = await import('./FlowMenu.svelte');
const { player } = await import('../stores/player.svelte');
const { ytm, reportRect, hideView, setOverlayOpen, applyState } = await import('../stores/ytm.svelte');
const { overlayOpen, setOverlayListener } = await import('../stores/overlays.svelte');
const { queuePanel } = await import('../stores/queue.svelte');
const { notice } = await import('../stores/notice.svelte');
const { setLang } = await import('../i18n');

const playing: YtmState = {
  status: 'playing',
  title: 'Glass Tides',
  artist: 'Lumen Field',
  album: 'Low Light',
  coverId: 'yt-0123456789abcdef0123',
  positionMs: 60_000,
  durationMs: 200_000,
  canNext: true,
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
const byLabel = (label: string) => target.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);

beforeEach(() => {
  setLang('en');
  invoke.mockClear();
  notice.text = null;
  queuePanel.open = false;
  player.active = 'ytm';
  Object.assign(player, { status: 'idle', track: null, positionMs: 0, durationMs: 0, queueLength: 0 });
  applyState({ ...playing });
  ytm.queue = { items: [], available: false };
  target = document.createElement('div');
  document.body.append(target);
});

afterEach(() => {
  for (const app of mounted.splice(0)) unmount(app);
  player.active = 'prism';
  target.remove();
});

describe('each_overlay_registers_while_open', () => {
  it('registers the palette while it is open', () => {
    const app = show(SearchPalette, { open: true });
    expect(overlayOpen('palette')).toBe(true);
    unmount(app);
    mounted.pop();
    expect(overlayOpen('palette')).toBe(false);
  });

  it('registers the track, station and Flow menus while mounted', () => {
    const track = show(TrackMenu, { trackIds: ['local:t1'], x: 10, y: 10, onclose: () => {} });
    expect(overlayOpen('track-menu')).toBe(true);
    const station = show(StationMenu, {
      station: { id: 'radio:s1', name: 'Radio Testhausen', homepage: null } as never,
      x: 10,
      y: 10,
      onclose: () => {},
    });
    expect(overlayOpen('station-menu')).toBe(true);
    const flow = show(FlowMenu, { x: 100, y: 100, onclose: () => {} });
    expect(overlayOpen('flow-menu')).toBe(true);
    for (const app of [track, station, flow]) {
      unmount(app);
      mounted.splice(mounted.indexOf(app), 1);
    }
    expect(overlayOpen('track-menu')).toBe(false);
    expect(overlayOpen('station-menu')).toBe(false);
    expect(overlayOpen('flow-menu')).toBe(false);
  });

  it('registers the queue panel while it is open', async () => {
    show(QueuePanel);
    expect(overlayOpen('queue')).toBe(false);
    queuePanel.open = true;
    await settle();
    expect(overlayOpen('queue')).toBe(true);
    queuePanel.open = false;
    await settle();
    expect(overlayOpen('queue')).toBe(false);
  });

  it('registers the notice while its text shows and the drop overlay while dragging', async () => {
    show(App);
    await settle();
    expect(overlayOpen('notice')).toBe(false);
    notice.text = 'Something happened';
    await settle();
    expect(overlayOpen('notice')).toBe(true);
    notice.text = null;
    await settle();
    expect(overlayOpen('notice')).toBe(false);

    await vi.waitFor(() => expect(dragHandler).toBeDefined());
    dragHandler!({ payload: { type: 'enter', paths: ['C:/demo/a.flac'] } });
    await settle();
    expect(overlayOpen('drop')).toBe(true);
    dragHandler!({ payload: { type: 'leave' } });
    await settle();
    expect(overlayOpen('drop')).toBe(false);
  });
});

describe('a replaced notice', () => {
  it('does not hide and show the YouTube Music view again', async () => {
    vi.stubGlobal('requestAnimationFrame', (cb: () => void) => setTimeout(cb, 0));
    show(App);
    await settle();
    // As the mounted YouTubeView does.
    setOverlayListener(setOverlayOpen);
    reportRect({ x: 1, y: 2, width: 300, height: 200 });
    await vi.waitFor(() => expect(calls('ytm_show')).toHaveLength(1));
    notice.text = 'First';
    await settle();
    expect(calls('ytm_hide')).toHaveLength(1);
    notice.text = 'Second';
    await settle();
    expect(calls('ytm_hide')).toHaveLength(1);
    expect(calls('ytm_show')).toHaveLength(1);
    expect(overlayOpen('notice')).toBe(true);
    notice.text = null;
    await settle();
    await vi.waitFor(() => expect(calls('ytm_show')).toHaveLength(2));
    hideView();
    setOverlayListener(null);
    vi.unstubAllGlobals();
  });
});

describe('transport strip with YouTube Music', () => {
  it('strip_shows_ytm_when_active', async () => {
    show(TransportStrip);
    await settle();
    expect(target.querySelector('.title')?.textContent?.trim()).toBe('Glass Tides');
    expect(target.querySelector('.sub')?.textContent?.trim()).toBe('Lumen Field');
    const label = target.querySelector<HTMLElement>('.quality')!;
    expect(label.textContent?.trim()).toBe('YouTube Music');
    expect(label.getAttribute('style')).toContain('--src-youtube');
    expect(target.querySelector('img')?.getAttribute('src')).toContain('yt-0123456789abcdef0123');
    expect(target.querySelector('.time.elapsed')?.textContent).toBe('1:00');
    expect(target.querySelector('[role="slider"][aria-label="Position"]')).not.toBeNull();
    // next and previous follow the page's buttons
    expect(byLabel('Next')!.disabled).toBe(false);
    expect(byLabel('Previous')!.disabled).toBe(true);
  });

  it('offers no heart and no play modes while YouTube Music is active', async () => {
    show(TransportStrip);
    await settle();
    expect(target.querySelector('.heart')).toBeNull();
    expect(target.querySelector('[aria-label^="Favorite"]')).toBeNull();
    expect(target.querySelector('[aria-label*="huffle"], [aria-label*="epeat"]')).toBeNull();
  });

  it('shows the heart again for a Prism track (control)', async () => {
    player.active = 'prism';
    Object.assign(player, {
      status: 'playing',
      track: { id: 'local:t1', source: 'local', title: 'Song', artist: 'A', album: 'B', albumId: 'a1', durationMs: 1000, format: 'flac', coverId: null, path: 'C:/a.flac' },
    });
    show(TransportStrip);
    await settle();
    expect(target.querySelector('.heart')).not.toBeNull();
  });

  it('seeking goes through the transport', async () => {
    show(TransportStrip);
    await settle();
    const slider = target.querySelector<HTMLElement>('[role="slider"][aria-label="Position"]')!;
    slider.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    await settle();
    expect(calls('player_seek')).toEqual([['player_seek', { ms: 65_000 }]]);
    // The drawn position jumps at once; the page's own report follows.
    expect(ytm.positionMs).toBe(65_000);
  });

  it('next and previous go through the player commands', async () => {
    applyState({ ...playing, canPrev: true });
    show(TransportStrip);
    await settle();
    byLabel('Next')!.click();
    byLabel('Previous')!.click();
    expect(calls('player_next')).toHaveLength(1);
    expect(calls('player_prev')).toHaveLength(1);
  });

  it('shows no seek value without a duration', async () => {
    applyState({ ...playing, durationMs: 0, positionMs: 0 });
    show(TransportStrip);
    await settle();
    expect(target.querySelector<HTMLElement>('[role="slider"][aria-label="Position"]')?.getAttribute('aria-disabled')).toBe('true');
  });

  it('volume uses the existing volume path', async () => {
    show(TransportStrip);
    await settle();
    const slider = target.querySelector<HTMLElement>('[role="slider"][aria-label="Volume"]')!;
    slider.dispatchEvent(new KeyboardEvent('keydown', { key: 'Home', bubbles: true }));
    await settle();
    expect(calls('player_set_volume').length).toBeGreaterThan(0);
  });
});

describe('Now Playing with YouTube Music', () => {
  it('shows the state, the amber source label and the queue note, without a heart', async () => {
    show(NowPlaying);
    await settle();
    expect(target.querySelector('h1')?.textContent?.trim()).toBe('Glass Tides');
    expect(target.querySelector('.artist')?.textContent?.trim()).toBe('Lumen Field');
    expect(target.querySelector('.quality')?.getAttribute('style')).toContain('--src-youtube');
    expect(target.querySelector('.quality')?.textContent?.trim()).toBe('YouTube Music');
    expect(target.querySelector('.ytm-note')?.textContent).toBe('Open the player in YouTube Music once to see its queue here.');
    expect(target.querySelector('[aria-label*="avorite"]')).toBeNull();
    expect(target.querySelector('[role="listbox"]')).toBeNull();
  });

  it('shows the YouTube Music queue and plays an entry on click or Enter', async () => {
    ytm.queue = ytQueue();
    show(NowPlaying);
    await settle();
    expect(target.querySelector('.ytm-note')).toBeNull();
    const rows = [...target.querySelectorAll<HTMLElement>('[role="option"]')];
    expect(rows.map((row) => row.querySelector('.qtitle')?.textContent)).toEqual(['Eins', 'Glass Tides', 'Drei']);
    expect(rows.map((row) => row.querySelector('.qartist')?.textContent)).toEqual(['Band A', 'Lumen Field', '']);
    expect(rows[1].classList.contains('current')).toBe(true);
    expect(rows[1].getAttribute('aria-current')).toBe('true');
    expect(rows.filter((row) => row.getAttribute('aria-current'))).toHaveLength(1);
    expect(target.querySelector('.pos')?.textContent).toContain('2');
    // No hearts, no remove buttons, nothing to drag.
    expect(target.querySelector('[aria-label*="avorite"]')).toBeNull();
    expect(target.querySelector('.remove')).toBeNull();

    rows[2].querySelector<HTMLElement>('.qtitle')!.click();
    await settle();
    expect(calls('ytm_queue_play')).toEqual([['ytm_queue_play', { index: 2 }]]);
    // The current entry already plays.
    rows[1].click();
    await settle();
    expect(calls('ytm_queue_play')).toHaveLength(1);

    const list = target.querySelector<HTMLElement>('[role="listbox"]')!;
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await settle();
    expect(calls('ytm_queue_play').at(-1)).toEqual(['ytm_queue_play', { index: 0 }]);
  });

  it('shows the note for an available but empty queue, and drops a selection past its end', async () => {
    ytm.queue = { available: true, items: [] };
    show(NowPlaying);
    await settle();
    expect(target.querySelector('[role="listbox"]')).toBeNull();
    expect(target.querySelector('.ytm-note')).not.toBeNull();

    ytm.queue = ytQueue();
    await settle();
    const list = target.querySelector<HTMLElement>('[role="listbox"]')!;
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    await settle();
    expect(list.getAttribute('aria-activedescendant')).toBe('ytnp-2');
    ytm.queue = { available: true, items: ytQueue().items.slice(0, 2) };
    await settle();
    expect(target.querySelector('[role="listbox"]')!.getAttribute('aria-activedescendant')).toBeNull();
  });

  it('shows the Prism queue again once Prism plays', async () => {
    ytm.queue = ytQueue();
    player.active = 'prism';
    show(NowPlaying);
    await settle();
    expect(target.textContent).not.toContain('Glass Tides');
  });
});

function ytQueue() {
  return {
    available: true,
    items: [
      { title: 'Eins', byline: 'Band A', coverId: 'yt-00000000000000000001', current: false },
      { title: 'Glass Tides', byline: 'Lumen Field', coverId: null, current: true },
      { title: 'Drei', byline: '', coverId: null, current: false },
    ],
  };
}

describe('queue panel with YouTube Music', () => {
  it('queue_panel_explains_ytm', async () => {
    queuePanel.open = true;
    show(QueuePanel);
    await settle();
    expect(target.querySelector('.empty p')?.textContent).toBe('Open the player in YouTube Music once to see its queue here.');
    expect(target.querySelector('[role="listbox"]')).toBeNull();
  });

  it('says the same in German', async () => {
    setLang('de');
    queuePanel.open = true;
    show(QueuePanel);
    await settle();
    expect(target.querySelector('.empty p')?.textContent).toBe(
      'Öffne einmal den Player in YouTube Music, dann erscheint die Warteschlange hier.',
    );
  });

  it('lists the YouTube Music queue and plays an entry on click or Enter', async () => {
    ytm.queue = ytQueue();
    queuePanel.open = true;
    show(QueuePanel);
    await settle();
    expect(target.querySelector('.panel > .empty')).toBeNull();
    const rows = [...target.querySelectorAll<HTMLElement>('[role="option"]')];
    expect(rows.map((row) => row.querySelector('.title')?.textContent)).toEqual(['Eins', 'Glass Tides', 'Drei']);
    // The current entry is marked like Prism's current row.
    expect(rows[1].classList.contains('current')).toBe(true);
    expect(rows[1].querySelector('.artist')?.textContent).toBe('Now playing');
    expect(rows[0].querySelector('.artist')?.textContent).toBe('Band A');
    expect(rows[0].querySelector('img, .cover')).not.toBeNull();
    expect(target.querySelector('.remove')).toBeNull();
    expect(target.querySelector('[aria-label*="avorite"]')).toBeNull();
    expect(target.querySelector('.row.upcoming')).toBeNull();

    rows[0].click();
    await settle();
    expect(calls('ytm_queue_play')).toEqual([['ytm_queue_play', { index: 0 }]]);
    // The row's play button plays it too.
    rows[2].querySelector<HTMLButtonElement>('button.play')!.click();
    await settle();
    expect(calls('ytm_queue_play').at(-1)).toEqual(['ytm_queue_play', { index: 2 }]);
    rows[1].click();
    await settle();
    expect(calls('ytm_queue_play')).toHaveLength(2);

    const list = target.querySelector<HTMLElement>('[role="listbox"]')!;
    // The last click selected the current entry; up and Enter play the one above it.
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await settle();
    expect(calls('ytm_queue_play')).toHaveLength(3);
    expect(calls('ytm_queue_play').at(-1)).toEqual(['ytm_queue_play', { index: 0 }]);
    expect(rows[0].getAttribute('aria-selected')).toBe('true');
    // Down to the current entry: Enter leaves it playing.
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await settle();
    expect(calls('ytm_queue_play')).toHaveLength(3);
    // Prism's own queue commands are never used for it.
    expect(calls('player_jump')).toHaveLength(0);
    expect(calls('player_queue_range')).toHaveLength(0);
  });

  it('shows the note for an available but empty queue', async () => {
    ytm.queue = { available: true, items: [] };
    queuePanel.open = true;
    show(QueuePanel);
    await settle();
    expect(target.querySelector('[role="listbox"]')).toBeNull();
    expect(target.querySelector('.empty p')?.textContent).toContain('Open the player');
  });

  it('drops a selection past the end of a shorter queue', async () => {
    ytm.queue = ytQueue();
    queuePanel.open = true;
    show(QueuePanel);
    await settle();
    const list = target.querySelector<HTMLElement>('[role="listbox"]')!;
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    await settle();
    expect(list.getAttribute('aria-activedescendant')).toBe('ytq-2');
    ytm.queue = { available: true, items: ytQueue().items.slice(0, 2) };
    await settle();
    expect(target.querySelector('[role="listbox"]')!.getAttribute('aria-activedescendant')).toBeNull();
    expect(target.querySelector('[aria-selected="true"]')).toBeNull();
    target.querySelector<HTMLElement>('[role="listbox"]')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await settle();
    expect(calls('ytm_queue_play')).toHaveLength(0);
  });

  it('follows the queue as it changes', async () => {
    ytm.queue = ytQueue();
    queuePanel.open = true;
    show(QueuePanel);
    await settle();
    ytm.queue = { available: true, items: [{ title: 'Neu', byline: 'X', coverId: null, current: true }] };
    await settle();
    expect([...target.querySelectorAll('[role="option"] .title')].map((el) => el.textContent)).toEqual(['Neu']);
    ytm.queue = { available: false, items: [] };
    await settle();
    expect(target.querySelector('.empty p')?.textContent).toContain('Open the player');
  });

  it('keeps the normal heading, not a leftover Flow name', async () => {
    player.flow = { kind: 'genre', genre: 'Ambient' };
    queuePanel.open = true;
    show(QueuePanel);
    await settle();
    expect(target.querySelector('h2')?.textContent).toBe('Up next');
    player.flow = null;
  });
});

describe('Flow ring with YouTube Music', () => {
  it('ring_shows_ytm_progress', async () => {
    show(FlowRing, { mode: 'pause', progress: 0.3, level: 0.9, size: 96 });
    await settle();
    const ring = target.querySelector<HTMLButtonElement>('.ring')!;
    expect(ring.getAttribute('aria-label')).toBe('Pause');
    expect(ring.style.getPropertyValue('--deg')).toBe('108.00deg');
    // a steady glow: no level meter, no breathing
    expect(ring.classList.contains('steady')).toBe(true);
    expect(ring.classList.contains('metered')).toBe(false);
    expect(ring.style.getPropertyValue('--glow-o')).toBe('');
  });

  it('keeps the level meter for Prism', async () => {
    player.active = 'prism';
    show(FlowRing, { mode: 'pause', progress: 0.3, level: 0.9, size: 96 });
    await settle();
    const ring = target.querySelector<HTMLButtonElement>('.ring')!;
    expect(ring.classList.contains('metered')).toBe(true);
    expect(ring.classList.contains('steady')).toBe(false);
  });
});
