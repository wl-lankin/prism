import { flushSync, mount, unmount } from 'svelte';
import { beforeEach, expect, it, vi } from 'vitest';

const invoke = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const { default: FlowMenu } = await import('./FlowMenu.svelte');
const { setLang } = await import('../i18n');

beforeEach(() => {
  setLang('en');
  invoke.mockReset();
  invoke.mockImplementation(async (...call: unknown[]) => {
    if (call[0] === 'library_genres') return ['Ambient', 'Electronic', 'Trip Hop'];
    if (call[0] === 'flow_start') return { outcome: 'started', variant: (call[1] as { variant: unknown }).variant };
    return undefined;
  });
});

function labels(target: HTMLElement): string[] {
  return [...target.querySelectorAll('[role="menuitem"]')].map((item) => item.textContent?.trim() ?? '');
}

it('offers the three variants and starts a library Flow', async () => {
  const target = document.createElement('div');
  document.body.append(target);
  const onclose = vi.fn();
  const menu = mount(FlowMenu, { target, props: { x: 200, y: 400, onclose } });
  flushSync();
  expect(labels(target)).toEqual(['Flow from favorites', 'Flow from the whole library', 'Flow from a genre']);
  target.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')[1].click();
  expect(onclose).toHaveBeenCalled();
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('flow_start', { variant: { kind: 'library' } }));
  unmount(menu);
  target.remove();
});

it('lists genres, filters them as you type and starts the one picked', async () => {
  const target = document.createElement('div');
  document.body.append(target);
  const onclose = vi.fn();
  const menu = mount(FlowMenu, { target, props: { x: 200, y: 400, onclose } });
  flushSync();
  target.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')[2].click();
  await vi.waitFor(() => {
    flushSync();
    expect(labels(target)).toEqual(['Ambient', 'Electronic', 'Trip Hop']);
  });
  const filter = target.querySelector<HTMLInputElement>('input')!;
  filter.value = 'trip';
  filter.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
  expect(labels(target)).toEqual(['Trip Hop']);
  filter.value = 'polka';
  filter.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
  expect(labels(target)).toEqual([]);
  expect(target.textContent).toContain('No genre matches');
  filter.value = 'tron';
  filter.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
  target.querySelector<HTMLButtonElement>('[role="menuitem"]')!.click();
  await vi.waitFor(() =>
    expect(invoke).toHaveBeenCalledWith('flow_start', { variant: { kind: 'genre', genre: 'Electronic' } }),
  );
  expect(onclose).toHaveBeenCalled();
  unmount(menu);
  target.remove();
});

it('closes on Escape', () => {
  const target = document.createElement('div');
  document.body.append(target);
  const onclose = vi.fn();
  const menu = mount(FlowMenu, { target, props: { x: 10, y: 10, onclose } });
  flushSync();
  target
    .querySelector('[role="menu"]')!
    .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  expect(onclose).toHaveBeenCalled();
  unmount(menu);
  target.remove();
});
