import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn(async (_cmd: string, _args?: Record<string, unknown>): Promise<unknown> => undefined);
vi.mock('@tauri-apps/api/core', () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

const { default: Sidebar } = await import('./Sidebar.svelte');
const { default: YouTubeView } = await import('./YouTubeView.svelte');
const { nav } = await import('../stores/nav.svelte');
const { ytm, hideView } = await import('../stores/ytm.svelte');
const { openOverlay, closeOverlay } = await import('../stores/overlays.svelte');

const HINT_KEY = 'prism.ytmSignInHintDismissed';
const RECT = { x: 220, y: 64, width: 800, height: 480 };

let target: HTMLElement;
let frames: Array<() => void> = [];
const mounted: ReturnType<typeof mount>[] = [];

const flushFrames = () => {
  const run = frames;
  frames = [];
  run.forEach((cb) => cb());
};
const calls = (cmd: string) => invoke.mock.calls.filter((call) => call[0] === cmd);
const button = (label: string) =>
  [...target.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === label);

function mountView() {
  const app = mount(YouTubeView, { target });
  mounted.push(app);
  flushSync();
  return app;
}

beforeEach(() => {
  invoke.mockClear();
  frames = [];
  localStorage.clear();
  vi.stubGlobal('requestAnimationFrame', (cb: () => void) => frames.push(cb));
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
    },
  );
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const on = this.classList.contains('stage');
    const [x, y, width, height] = on ? [RECT.x, RECT.y, RECT.width, RECT.height] : [0, 0, 0, 0];
    return { x, y, width, height, left: x, top: y, right: x + width, bottom: y + height, toJSON: () => ({}) };
  });
  nav.view = 'home';
  nav.param = undefined;
  nav.source = undefined;
  target = document.createElement('div');
  document.body.append(target);
});

afterEach(() => {
  for (const app of mounted.splice(0)) unmount(app);
  hideView();
  closeOverlay('test');
  target.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('YouTube Music view', () => {
  it('sidebar_shows_youtube_music', () => {
    mounted.push(mount(Sidebar, { target }));
    flushSync();
    const labels = [...target.querySelectorAll('button.item .label')].map((el) => el.textContent);
    const at = labels.indexOf('YouTube Music');
    expect(at).toBeGreaterThan(-1);
    expect(labels[at - 1]).toBe('Navidrome');
    expect(labels[at + 1]).toBe('Radio');
    const entry = target.querySelectorAll<HTMLButtonElement>('button.item')[at];
    expect(entry.getAttribute('aria-disabled')).toBeNull();
    expect(entry.querySelector<HTMLElement>('.dot')?.getAttribute('style')).toContain('--src-youtube');
    entry.click();
    flushSync();
    expect(nav.view).toBe('youtube');
    expect(entry.getAttribute('aria-current')).toBe('page');
  });

  it('view_reports_its_rect_and_hides_on_leave', () => {
    const app = mountView();
    flushFrames();
    expect(calls('ytm_show')).toHaveLength(1);
    expect(calls('ytm_show')[0][1]).toEqual({ rect: RECT });
    expect(calls('ytm_hide')).toHaveLength(0);
    unmount(app);
    mounted.splice(mounted.indexOf(app), 1);
    expect(calls('ytm_hide')).toHaveLength(1);
    expect(ytm.shown).toBe(false);
  });

  it('window_resize_reports_again', () => {
    mountView();
    flushFrames();
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      const on = this.classList.contains('stage');
      const [x, y, width, height] = on ? [220, 64, 640, 480] : [0, 0, 0, 0];
      return { x, y, width, height, left: x, top: y, right: x + width, bottom: y + height, toJSON: () => ({}) };
    });
    window.dispatchEvent(new Event('resize'));
    flushFrames();
    expect(calls('ytm_show')).toHaveLength(2);
    expect(calls('ytm_show')[1][1]).toEqual({ rect: { ...RECT, width: 640 } });
  });

  it('sign_in_button_calls_the_backend', () => {
    mountView();
    button('Sign in')!.click();
    expect(calls('ytm_sign_in')).toHaveLength(1);
    flushSync();
    expect(target.querySelector('.hint')).toBeNull();
    expect(localStorage.getItem(HINT_KEY)).toBe('1');
  });

  it('the_still_shows_while_hidden_for_an_overlay', async () => {
    mountView();
    flushFrames();
    expect(target.querySelector('.still')).toBeNull();
    openOverlay('test');
    await tick();
    expect(ytm.hiddenForOverlay).toBe(true);
    expect(target.querySelector('.still')).not.toBeNull();
    // No artwork yet: the neutral gradient, no image.
    expect(target.querySelector('.still img')).toBeNull();
    ytm.state = { ...ytm.state, coverId: 'yt-abc' };
    await tick();
    expect(target.querySelector('.still img')?.getAttribute('src')).toContain('/cover/yt-abc/600');
    closeOverlay('test');
    await tick();
    expect(target.querySelector('.still')).toBeNull();
    ytm.state = { ...ytm.state, coverId: null };
  });

  it('hint_dismissal_is_remembered', () => {
    const app = mountView();
    expect(target.querySelector('.hint')).not.toBeNull();
    button('Dismiss')!.click();
    flushSync();
    expect(target.querySelector('.hint')).toBeNull();
    expect(localStorage.getItem(HINT_KEY)).toBe('1');
    unmount(app);
    mounted.splice(mounted.indexOf(app), 1);
    mountView();
    expect(target.querySelector('.hint')).toBeNull();
  });

  it('a_blocked_storage_does_not_break_the_hint', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    mountView();
    expect(target.querySelector('.hint')).not.toBeNull();
    button('Dismiss')!.click();
    flushSync();
    expect(target.querySelector('.hint')).toBeNull();
  });

  it('a_hairline_shows_while_loading', () => {
    mountView();
    expect(target.querySelector('.hairline')).not.toBeNull();
    expect(target.querySelector('.stage')?.getAttribute('aria-busy')).toBe('true');
    openOverlay('test');
    flushSync();
    expect(target.querySelector('.hairline')).toBeNull();
    closeOverlay('test');
    flushSync();
    expect(target.querySelector('.hairline')).not.toBeNull();
    ytm.state = { ...ytm.state, status: 'playing', title: 'Demo' };
    flushSync();
    expect(target.querySelector('.hairline')).toBeNull();
    ytm.state = { ...ytm.state, status: 'idle', title: '' };
  });
});
