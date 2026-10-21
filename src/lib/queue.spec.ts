import { afterEach, beforeEach, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => undefined) }));

const { dragTarget, firstRow, stepAside, QUEUE_PANEL_KEY } = await import('./queue');
const { queuePanel, loadQueuePanel, toggleQueuePanel } = await import('./stores/queue.svelte');

it('starts the list at the current entry unless the played ones are shown', () => {
  expect(firstRow(4, 10, false)).toBe(4);
  expect(firstRow(4, 10, true)).toBe(0);
  expect(firstRow(0, 0, false)).toBe(0);
  expect(firstRow(12, 10, false)).toBe(10);
});

it('drops a dragged row on whole rows inside the allowed range', () => {
  // Upcoming entries are 5..9 (the current one is 4).
  expect(dragTarget(7, 0, 52, 5, 9)).toBe(7);
  expect(dragTarget(7, 52 * 2 - 10, 52, 5, 9)).toBe(9);
  expect(dragTarget(7, 52 * 9, 52, 5, 9)).toBe(9);
  // Never onto the current or the played ones.
  expect(dragTarget(7, -52 * 9, 52, 5, 9)).toBe(5);
  expect(dragTarget(7, -30, 52, 5, 9)).toBe(6);
});

it('moves the rows a drag passes by one row', () => {
  const drag = { from: 6, to: 8, dy: 100 };
  expect(stepAside(6, drag, 52)).toBe(100);
  expect(stepAside(7, drag, 52)).toBe(-52);
  expect(stepAside(8, drag, 52)).toBe(-52);
  expect(stepAside(9, drag, 52)).toBe(0);
  const up = { from: 8, to: 6, dy: -100 };
  expect(stepAside(6, up, 52)).toBe(52);
  expect(stepAside(7, up, 52)).toBe(52);
  expect(stepAside(5, up, 52)).toBe(0);
  expect(stepAside(5, null, 52)).toBe(0);
});

beforeEach(() => {
  localStorage.clear();
  queuePanel.open = false;
});

afterEach(() => vi.restoreAllMocks());

it('remembers whether the queue panel is open', () => {
  loadQueuePanel();
  expect(queuePanel.open).toBe(false);
  toggleQueuePanel();
  expect(queuePanel.open).toBe(true);
  expect(localStorage.getItem(QUEUE_PANEL_KEY)).toBe('1');
  queuePanel.open = false;
  loadQueuePanel();
  expect(queuePanel.open).toBe(true);
  toggleQueuePanel();
  expect(localStorage.getItem(QUEUE_PANEL_KEY)).toBe('0');
});

it('works without storage', () => {
  vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
    throw new Error('blocked');
  });
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('blocked');
  });
  loadQueuePanel();
  expect(queuePanel.open).toBe(false);
  toggleQueuePanel();
  expect(queuePanel.open).toBe(true);
});
