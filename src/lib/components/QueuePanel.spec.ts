import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { PlayerState, QueueItem, Track } from '../types';

function track(n: number): Track {
  return {
    id: `t${n}`,
    source: 'local',
    title: `Song ${n}`,
    artist: 'Lumen Field',
    albumArtist: 'Lumen Field',
    album: 'Low Light',
    albumId: 'a1',
    trackNo: n,
    discNo: 1,
    durationMs: 200_000,
    year: 2021,
    genre: null,
    format: 'flac',
    sampleRate: 44_100,
    bitDepth: 16,
    bitrateKbps: null,
    coverId: null,
    path: `C:/demo/${n}.flac`,
    replayGainTrackDb: null,
    replayGainAlbumDb: null,
    replayGainTrackPeak: null,
    replayGainAlbumPeak: null,
  };
}

const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>): Promise<unknown> => {
  if (cmd === 'player_queue_range') {
    const { offset, limit } = args as { offset: number; limit: number };
    const items: QueueItem[] = [];
    for (let i = offset; i < Math.min(8, offset + limit); i++) items.push({ index: i, track: track(i) });
    return items;
  }
  if (cmd === 'favorites_ids') return { tracks: [], albums: [], artists: [] };
  return undefined;
});
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
  convertFileSrc: (path: string) => path,
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

// jsdom has no layout: `bind:clientHeight` needs the observer to exist, and the list shows
// its overscan rows.
vi.stubGlobal(
  'ResizeObserver',
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);

const { default: QueuePanel } = await import('./QueuePanel.svelte');
const { player } = await import('../stores/player.svelte');
const { queuePanel } = await import('../stores/queue.svelte');
const { setLang } = await import('../i18n');

const playing: Partial<PlayerState> = {
  status: 'playing',
  track: track(2),
  queueIndex: 2,
  queueLength: 8,
  queueVersion: 1,
  flow: null,
  repeat: 'off',
  shuffle: false,
};

let component: ReturnType<typeof mount> | undefined;
let target: HTMLElement;

async function settle() {
  for (let i = 0; i < 4; i++) {
    await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
    await Promise.resolve();
    flushSync();
  }
}

async function render() {
  component = mount(QueuePanel, { target });
  await settle();
}

const titles = () => [...target.querySelectorAll('.row .title')].map((el) => el.textContent);
const list = () => target.querySelector<HTMLElement>('[role="listbox"]')!;

beforeEach(() => {
  setLang('en');
  invoke.mockClear();
  Object.assign(player, playing);
  queuePanel.open = true;
  target = document.createElement('div');
  document.body.append(target);
});

afterEach(() => {
  if (component) unmount(component);
  component = undefined;
  target.remove();
});

it('starts at the playing track, lit, with the upcoming ones after it', async () => {
  await render();
  expect(titles()).toEqual(['Song 2', 'Song 3', 'Song 4', 'Song 5', 'Song 6', 'Song 7']);
  const first = target.querySelector('.row')!;
  expect(first.classList.contains('current')).toBe(true);
  expect(first.querySelector('.remove')).toBeNull();
  expect(target.querySelector('h2')!.textContent).toBe('Up next');
  expect(target.querySelector('.sub')!.textContent).toBe('5 songs after this');
});

it('shows the played tracks above on request', async () => {
  await render();
  const toggle = target.querySelector<HTMLButtonElement>('.played')!;
  expect(toggle.textContent).toContain('Show played (2)');
  expect(toggle.getAttribute('aria-expanded')).toBe('false');
  toggle.click();
  await settle();
  expect(titles().slice(0, 3)).toEqual(['Song 0', 'Song 1', 'Song 2']);
  expect(target.querySelector('.row')!.classList.contains('past')).toBe(true);
});

it('jumps on a click and removes with the X or Delete', async () => {
  await render();
  const rows = target.querySelectorAll<HTMLElement>('.row');
  rows[2].click();
  expect(invoke).toHaveBeenCalledWith('player_jump', { index: 4 });
  rows[1].querySelector<HTMLButtonElement>('.remove')!.click();
  expect(invoke).toHaveBeenCalledWith('player_remove', { index: 3 });
  await settle();
  expect(titles().slice(0, 3)).toEqual(['Song 2', 'Song 4', 'Song 5']);

  // The selection followed "Song 4" to index 3; the next row is index 4.
  invoke.mockClear();
  list().dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
  flushSync();
  list().dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }));
  expect(invoke).toHaveBeenCalledWith('player_remove', { index: 4 });
});

it('never removes the playing track', async () => {
  await render();
  list().dispatchEvent(new KeyboardEvent('keydown', { key: 'Home', bubbles: true }));
  flushSync();
  list().dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }));
  expect(invoke).not.toHaveBeenCalledWith('player_remove', expect.anything());
});

it('moves upcoming tracks with Alt+arrows, never above the playing one', async () => {
  await render();
  list().dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
  list().dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true }));
  expect(invoke).toHaveBeenCalledWith('player_move', { from: 3, to: 4 });
  await settle();
  expect(titles().slice(0, 3)).toEqual(['Song 2', 'Song 4', 'Song 3']);
  invoke.mockClear();
  list().dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
  list().dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', altKey: true, bubbles: true }));
  expect(invoke).not.toHaveBeenCalledWith('player_move', expect.anything());
});

it('closes with Escape and is inert while closed', async () => {
  await render();
  list().dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  flushSync();
  expect(queuePanel.open).toBe(false);
  const panel = target.querySelector('aside')!;
  expect(panel.classList.contains('open')).toBe(false);
  expect(panel.hasAttribute('inert') || (panel as HTMLElement & { inert?: boolean }).inert).toBe(true);
});

it('closes with Escape from the close button and the played toggle too', async () => {
  await render();
  target.querySelector('.close')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  flushSync();
  expect(queuePanel.open).toBe(false);
  queuePanel.open = true;
  await settle();
  target.querySelector('.played')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  flushSync();
  expect(queuePanel.open).toBe(false);
});

it('moves focus onto the playing row when it opens, not when it starts open', async () => {
  await render();
  expect(document.activeElement).not.toBe(list());
  queuePanel.open = false;
  await settle();
  queuePanel.open = true;
  await settle();
  expect(document.activeElement).toBe(list());
  expect(list().getAttribute('aria-activedescendant')).toBe('upnext-2');
});

it('hands focus back to the queue toggle when it closes with focus inside', async () => {
  const toggle = document.createElement('button');
  toggle.id = 'queue-toggle';
  document.body.append(toggle);
  try {
    await render();
    list().focus();
    list().dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    flushSync();
    expect(document.activeElement).toBe(toggle);

    // Closed from outside (the toggle itself): focus stays where it is.
    queuePanel.open = true;
    await settle();
    toggle.focus();
    queuePanel.open = false;
    flushSync();
    expect(document.activeElement).toBe(toggle);
  } finally {
    toggle.remove();
  }
});

it('names the Flow in its heading and leaves out the end note', async () => {
  Object.assign(player, { flow: { kind: 'genre', genre: 'Jazz' }, queueIndex: 7 });
  await render();
  const heading = target.querySelector('h2')!;
  expect(heading.textContent).toBe('Flow · Jazz');
  expect(heading.classList.contains('flow')).toBe(true);
  expect(target.querySelector('.tail')).toBeNull();
});

it('says what follows the last track', async () => {
  Object.assign(player, { queueIndex: 7 });
  await render();
  expect(target.querySelector('.tail')!.textContent).toBe('This is the last song.');
  player.repeat = 'all';
  flushSync();
  expect(target.querySelector('.tail')!.textContent).toMatch(/starts over/);
  player.repeat = 'one';
  flushSync();
  expect(target.querySelector('.tail')!.textContent).toBe('This song repeats.');
  setLang('de');
  flushSync();
  expect(target.querySelector('.tail')!.textContent).toBe('Dieser Song wiederholt sich.');
});

it('shows an empty state with nothing playing', async () => {
  Object.assign(player, { status: 'idle', track: null, queueLength: 0 });
  await render();
  expect(target.textContent).toContain('Nothing up next');
  expect(invoke).not.toHaveBeenCalledWith('player_queue_range', expect.anything());
});
