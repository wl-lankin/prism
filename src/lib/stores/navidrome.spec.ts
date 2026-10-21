import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { NavidromeErrorCode, NavidromeStatus } from '../types';

type Handler = (event: { event: string; id: number; payload: unknown }) => void;

const listeners = new Map<string, Handler>();

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (event: string, handler: Handler) => {
    listeners.set(event, handler);
    return () => listeners.delete(event);
  }),
}));

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const { navidrome, initNavidrome, failureText, isNavidromeError, navidromeErrorText, NAVIDROME_ERROR_CODES } =
  await import('./navidrome.svelte');
const { setLang, strings } = await import('../i18n');

const status = (patch: Partial<NavidromeStatus>): NavidromeStatus => ({
  configured: true,
  url: 'https://music.example.com',
  user: 'demo',
  state: 'online',
  lastSync: 1_700_000_000_000,
  trackCount: 12,
  albumCount: 3,
  progress: null,
  error: null,
  ...patch,
});

const send = (payload: NavidromeStatus) =>
  listeners.get('navidrome://status')!({ event: 'navidrome://status', id: 1, payload });

/** Every code the backend can send: client errors plus the service's own. */
const BACKEND_CODES: NavidromeErrorCode[] = [
  'invalidUrl',
  'wrongCredentials',
  'tokenAuthUnsupported',
  'notAuthorized',
  'notFound',
  'unreachable',
  'tls',
  'notSubsonic',
  'server',
  'syncRefused',
  'keychain',
  'database',
  'settings',
  'internal',
];

beforeEach(() => {
  listeners.clear();
  invoke.mockReset();
  setLang('en');
});

afterEach(() => setLang('en'));

it('starts off', () => {
  expect(navidrome.state).toBe('off');
  expect(navidrome.configured).toBe(false);
});

it('loads the status and follows navidrome://status', async () => {
  invoke.mockResolvedValue(status({ state: 'syncing' }));
  const stop = await initNavidrome();
  expect(invoke).toHaveBeenCalledWith('navidrome_status', undefined);
  expect(navidrome.state).toBe('syncing');
  expect(navidrome.user).toBe('demo');

  send(status({ state: 'offline', trackCount: 40 }));
  expect(navidrome.state).toBe('offline');
  expect(navidrome.trackCount).toBe(40);

  send(status({ state: 'syncing', progress: { done: 500, total: 1000 } }));
  expect(navidrome.progress).toEqual({ done: 500, total: 1000 });
  stop();
  expect(listeners.has('navidrome://status')).toBe(false);
});

it('keeps an event that arrived while the first status was on its way', async () => {
  let answer: (value: NavidromeStatus) => void = () => {};
  invoke.mockReturnValue(new Promise<NavidromeStatus>((resolve) => (answer = resolve)));
  const started = initNavidrome();
  await vi.waitFor(() => expect(listeners.has('navidrome://status')).toBe(true));
  send(status({ state: 'online', trackCount: 99 }));
  answer(status({ state: 'syncing', trackCount: 1 }));
  const stop = await started;
  expect(navidrome.state).toBe('online');
  expect(navidrome.trackCount).toBe(99);
  stop();
});

it('covers exactly the codes the backend sends', () => {
  expect([...NAVIDROME_ERROR_CODES].sort()).toEqual([...BACKEND_CODES].sort());
});

it('gives a text for every code in both languages', () => {
  for (const lang of ['en', 'de'] as const) {
    setLang(lang);
    for (const code of BACKEND_CODES) {
      const text = failureText({ code, detail: null });
      expect(text.trim().length, `${lang} ${code}`).toBeGreaterThan(0);
      expect(text, `${lang} ${code}`).not.toContain('navidrome.error');
      expect(text, `${lang} ${code}`).toBe(strings[lang][`navidrome.error.${code}`]);
    }
  }
});

it('appends the detail for server and notSubsonic only', () => {
  expect(failureText({ code: 'server', detail: 'Database is locked' })).toContain('Database is locked');
  expect(failureText({ code: 'notSubsonic', detail: 'HTTP 401' })).toContain('HTTP 401');
  expect(failureText({ code: 'unreachable', detail: 'connection refused' })).not.toContain('connection refused');
  expect(failureText({ code: 'database', detail: 'disk I/O error' })).not.toContain('disk I/O');
});

it('says clearly when the keychain refuses a value that is too long', () => {
  const plain = failureText({ code: 'keychain', detail: 'saving failed: the keychain failed' });
  const long = failureText({ code: 'keychain', detail: 'saving failed: a value is too long for the keychain' });
  expect(plain).toBe(strings.en['navidrome.error.keychain']);
  expect(long).toBe(strings.en['navidrome.error.keychainTooLong']);
  setLang('de');
  expect(failureText({ code: 'keychain', detail: 'a value is too long for the keychain' })).toBe(
    strings.de['navidrome.error.keychainTooLong'],
  );
});

it('falls back to a generic text for a code it does not know', () => {
  const text = failureText({ code: 'later' as NavidromeErrorCode, detail: 'x' });
  expect(text).toBe(strings.en['navidrome.error.internal']);
});

it('recognizes a NavidromeFailure', () => {
  expect(isNavidromeError({ code: 'tls', detail: 'unknown issuer' })).toBe(true);
  expect(isNavidromeError({ code: 'wrongCredentials', detail: null })).toBe(true);
  expect(isNavidromeError({ code: 'later', detail: null })).toBe(false);
  expect(isNavidromeError({ code: 'tls', detail: 3 })).toBe(false);
  expect(isNavidromeError('Navidrome is not connected')).toBe(false);
  expect(isNavidromeError(null)).toBe(false);
  expect(isNavidromeError(undefined)).toBe(false);
});

it('turns any Navidrome command error into a text', () => {
  expect(navidromeErrorText({ code: 'wrongCredentials', detail: null })).toBe(
    strings.en['navidrome.error.wrongCredentials'],
  );
  expect(navidromeErrorText('Navidrome is not connected')).toBe(strings.en['navidrome.notConnected']);
  expect(navidromeErrorText('disk full')).toBe('disk full');
});
