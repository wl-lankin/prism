import { afterEach, expect, it, vi } from 'vitest';
import { coverUrl } from './ipc';

afterEach(() => vi.unstubAllGlobals());

it('uses the http .localhost form on Windows', () => {
  vi.stubGlobal('navigator', { userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' });
  expect(coverUrl('abc123', 96)).toBe('http://prism.localhost/cover/abc123/96');
});

it('uses the prism scheme elsewhere', () => {
  vi.stubGlobal('navigator', { userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)' });
  expect(coverUrl('abc123', 300)).toBe('prism://localhost/cover/abc123/300');
});

it('returns null without an id', () => {
  expect(coverUrl(null, 600)).toBeNull();
  expect(coverUrl('', 600)).toBeNull();
});

it('treats Android like Windows and encodes the id', () => {
  vi.stubGlobal('navigator', { userAgent: 'Mozilla/5.0 (Linux; Android 14)' });
  expect(coverUrl('a/b c', 96)).toBe('http://prism.localhost/cover/a%2Fb%20c/96');
});
