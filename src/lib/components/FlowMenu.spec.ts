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

it('hands the pick to onpick instead of starting it', async () => {
  const target = document.createElement('div');
  document.body.append(target);
  const onpick = vi.fn();
  const onclose = vi.fn();
  const menu = mount(FlowMenu, { target, props: { x: 200, y: 400, onclose, onpick } });
  flushSync();
  target.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')[0].click();
  expect(onpick).toHaveBeenCalledWith({ kind: 'favorites' });
  expect(onclose).toHaveBeenCalled();
  await Promise.resolve();
  expect(invoke).not.toHaveBeenCalledWith('flow_start', expect.anything());
  unmount(menu);
  target.remove();
});

it('opens above the point anchored by its bottom edge, capped to the room above', async () => {
  const target = document.createElement('div');
  document.body.append(target);
  const menu = mount(FlowMenu, { target, props: { x: 200, y: 500, onclose: () => {} } });
  flushSync();
  const box = target.querySelector<HTMLElement>('[role="menu"]')!;
  await vi.waitFor(() => {
    flushSync();
    expect(box.style.bottom).toBe(`${window.innerHeight - 500 + 8}px`);
  });
  expect(box.style.top).toBe('');
  expect(box.style.maxHeight).toBe(`${500 - 16}px`);
  unmount(menu);
  target.remove();
});

it('opens below a point near the top when the whole menu does not fit above', async () => {
  // jsdom has no layout: the menu is 124 px tall, less while its max-height caps it.
  const natural = 124;
  const rect = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const cap = parseFloat(this.style.maxHeight);
    const height = Number.isNaN(cap) ? natural : Math.min(natural, cap);
    return { x: 0, y: 0, left: 0, top: 0, right: 300, bottom: height, width: 300, height, toJSON() {} } as DOMRect;
  });
  const target = document.createElement('div');
  document.body.append(target);
  const menu = mount(FlowMenu, { target, props: { x: 66, y: 60, onclose: () => {} } });
  flushSync();
  const box = target.querySelector<HTMLElement>('[role="menu"]')!;
  await vi.waitFor(() => {
    flushSync();
    expect(box.style.top).toBe(`${60 + 8}px`);
  });
  expect(box.style.bottom).toBe('');
  expect(box.style.maxHeight).toBe(`${window.innerHeight - 60 - 16}px`);
  unmount(menu);
  target.remove();
  rect.mockRestore();
});

it('gives focus back to the opener and closes when focus leaves', async () => {
  const ring = document.createElement('button');
  const elsewhere = document.createElement('button');
  document.body.append(ring, elsewhere);
  ring.focus();
  const target = document.createElement('div');
  document.body.append(target);
  const onclose = vi.fn();
  const menu = mount(FlowMenu, { target, props: { x: 200, y: 400, onclose } });
  flushSync();
  const first = target.querySelector<HTMLButtonElement>('[role="menuitem"]')!;
  await vi.waitFor(() => expect(document.activeElement).toBe(first));
  elsewhere.focus();
  expect(onclose).toHaveBeenCalled();
  first.focus();
  unmount(menu);
  expect(document.activeElement).toBe(ring);
  target.remove();
  ring.remove();
  elsewhere.remove();
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
