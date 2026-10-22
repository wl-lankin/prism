import { afterEach, beforeEach, expect, it, vi } from 'vitest';

const invoke = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const { HOLD_MS, activateIdle, holdToOpen, playFlow, shuffle } = await import('./flow');
const { notice } = await import('./stores/notice.svelte');
const { nav } = await import('./stores/nav.svelte');
const { setLang, strings } = await import('./i18n');

/** `flow_start` answers with `outcome`; every other command resolves empty. */
function answer(outcome: unknown) {
  invoke.mockImplementation(async (...call: unknown[]) => (call[0] === 'flow_start' ? outcome : undefined));
}

beforeEach(() => {
  setLang('en');
  invoke.mockReset();
  notice.text = null;
  nav.view = 'home';
  nav.param = undefined;
});

afterEach(() => {
  vi.useRealTimers();
});

it('starts a favorites Flow on an idle activation', async () => {
  answer({ outcome: 'started', variant: { kind: 'favorites' } });
  expect(await activateIdle()).toBe('started');
  expect(invoke).toHaveBeenCalledWith('flow_start', { variant: { kind: 'favorites' } });
  expect(notice.text).toBeNull();
});

it('says so when favorites fall back to the library', async () => {
  answer({ outcome: 'started', variant: { kind: 'library' } });
  expect(await activateIdle()).toBe('started');
  expect(invoke).toHaveBeenCalledWith('flow_start', { variant: { kind: 'favorites' } });
  expect(notice.text).toBe(strings.en['flow.noFavorites']);
});

it('goes to Settings when the library is empty', async () => {
  answer({ outcome: 'empty' });
  expect(await activateIdle()).toBe('empty');
  expect(nav.view).toBe('settings');
});

it('starts a genre Flow as asked, without a notice', async () => {
  answer({ outcome: 'started', variant: { kind: 'genre', genre: 'Trip Hop' } });
  await playFlow({ kind: 'genre', genre: 'Trip Hop' });
  expect(invoke).toHaveBeenCalledWith('flow_start', { variant: { kind: 'genre', genre: 'Trip Hop' } });
  expect(notice.text).toBeNull();
});

it('shows the backend message when the Flow cannot start', async () => {
  invoke.mockImplementation(async () => {
    throw 'The audio engine is not running.';
  });
  expect(await activateIdle()).toBe('failed');
  expect(notice.text).toContain('The audio engine is not running.');
});

it('opens on a long press and swallows the click that ends it', () => {
  vi.useFakeTimers();
  const node = document.createElement('div');
  const button = document.createElement('button');
  node.append(button);
  const onhold = vi.fn();
  const clicked = vi.fn();
  button.addEventListener('click', clicked);
  const action = holdToOpen(node, onhold);

  // A second finger (not the primary pointer) never opens it.
  button.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, isPrimary: false }));
  vi.advanceTimersByTime(HOLD_MS * 2);
  expect(onhold).not.toHaveBeenCalled();

  button.dispatchEvent(
    new PointerEvent('pointerdown', { bubbles: true, button: 0, isPrimary: true, clientX: 40, clientY: 50 }),
  );
  vi.advanceTimersByTime(HOLD_MS - 1);
  expect(onhold).not.toHaveBeenCalled();
  vi.advanceTimersByTime(1);
  expect(onhold).toHaveBeenCalledWith(40, 50);
  button.dispatchEvent(new PointerEvent('pointerup', { bubbles: true }));
  button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  expect(clicked).not.toHaveBeenCalled();

  // A short press is a plain click.
  button.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, isPrimary: true }));
  vi.advanceTimersByTime(200);
  button.dispatchEvent(new PointerEvent('pointerup', { bubbles: true }));
  button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  vi.advanceTimersByTime(HOLD_MS);
  expect(clicked).toHaveBeenCalledTimes(1);
  expect(onhold).toHaveBeenCalledTimes(1);
  action.destroy();
});

it('shuffles without losing or duplicating items', () => {
  const items = Array.from({ length: 50 }, (_, i) => i);
  const out = shuffle([...items]);
  expect([...out].sort((a, b) => a - b)).toEqual(items);
});
