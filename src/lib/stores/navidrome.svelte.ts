import { t } from '../i18n';
import { navidromeStatus, onNavidromeStatus } from '../ipc';
import type { NavidromeErrorCode, NavidromeFailure, NavidromeStatus } from '../types';

/** The Navidrome account as the UI shows it; `off` until the backend answers. Replaced
 *  field by field from `navidrome_status` and every `navidrome://status`. */
export const navidrome: NavidromeStatus = $state({
  configured: false,
  url: null,
  user: null,
  state: 'off',
  lastSync: null,
  trackCount: 0,
  albumCount: 0,
  progress: null,
  error: null,
});

/** Every failure code the backend sends (`client.rs` `NavidromeError::code` and the
 *  `FAILURE_*` constants of `navidrome/mod.rs`). */
export const NAVIDROME_ERROR_CODES: readonly NavidromeErrorCode[] = [
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
  'keychainTooLong',
  'database',
  'settings',
  'internal',
];

const KNOWN = new Set<string>(NAVIDROME_ERROR_CODES);

/** Codes whose `detail` is the server's own words and worth showing. */
const WITH_DETAIL = new Set<NavidromeErrorCode>(['server', 'notSubsonic']);

/** What a heart or a stream answers while no account is signed in: a plain message, not a
 *  `NavidromeFailure`. The one copy of the backend's `navidrome::NOT_CONNECTED`, whose exact
 *  text the Rust tests pin. */
export const NAVIDROME_NOT_CONNECTED = 'Navidrome is not connected';

/** Whether `navidrome` holds an answer yet: set by the first status answer, the first event,
 *  or a failed load. Until then `state: 'off'` is only the starting value, not a sign-out. */
export const navidromeStatusKnown = $state({ loaded: false });

function apply(status: NavidromeStatus): void {
  Object.assign(navidrome, status);
  navidromeStatusKnown.loaded = true;
}

/** Loads the status, then follows `navidrome://status`. Resolves with the cleanup. */
export async function initNavidrome(): Promise<() => void> {
  // An event that arrives while the first status is on its way is newer than that answer.
  let heard = false;
  let stop: (() => void) | undefined;
  try {
    stop = await onNavidromeStatus((status) => {
      if (!status) return;
      heard = true;
      apply(status);
    });
  } catch (err) {
    console.error('following the Navidrome status failed', err);
  }
  try {
    const status = await navidromeStatus();
    if (status && !heard) apply(status);
  } catch (err) {
    console.error('loading the Navidrome status failed', err);
  }
  // Also after a failed or empty answer: the hearts then go by the starting state.
  navidromeStatusKnown.loaded = true;
  return () => stop?.();
}

/** True for what `navidrome_test` / `navidrome_connect` reject with. */
export function isNavidromeError(err: unknown): err is NavidromeFailure {
  if (typeof err !== 'object' || err === null) return false;
  const { code, detail } = err as { code?: unknown; detail?: unknown };
  const detailOk = detail === null || detail === undefined || typeof detail === 'string';
  return typeof code === 'string' && KNOWN.has(code) && detailOk;
}

/** The failure in the UI language. The server's own words are appended for `server` and
 *  `notSubsonic`; other details are technical English and stay out. */
export function failureText(failure: NavidromeFailure): string {
  const code: NavidromeErrorCode = KNOWN.has(failure.code) ? failure.code : 'internal';
  const text = t(`navidrome.error.${code}`);
  const detail = failure.detail?.trim();
  return detail && WITH_DETAIL.has(code) ? t('navidrome.error.withDetail', { text, detail }) : text;
}

/** Any rejection of a Navidrome command (or of a heart on a Navidrome item) as a text. An
 *  object with a string `code` counts as a failure even when the code is newer than this
 *  build (`failureText` shows the generic text for it). */
export function navidromeErrorText(err: unknown): string {
  const fields = (typeof err === 'object' && err !== null ? err : {}) as { code?: unknown; detail?: unknown };
  if (typeof fields.code === 'string') {
    const detail = typeof fields.detail === 'string' ? fields.detail : null;
    return failureText({ code: fields.code as NavidromeErrorCode, detail });
  }
  const message = err instanceof Error ? err.message : String(err);
  return message === NAVIDROME_NOT_CONNECTED ? t('navidrome.notConnected') : message;
}
