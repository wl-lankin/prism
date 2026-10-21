import { afterEach, expect, it, vi } from 'vitest';

const invoke = vi.fn(async (..._args: unknown[]) => undefined);
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
const listen = vi.fn(async (..._args: unknown[]) => () => {});
vi.mock('@tauri-apps/api/event', () => ({ listen: (...args: unknown[]) => listen(...args) }));

const ipc = await import('./ipc');
const { coverUrl } = ipc;

it('maps every command onto its fixed name and argument keys', async () => {
  const calls: [() => Promise<unknown>, string, Record<string, unknown> | undefined][] = [
    [() => ipc.playerPlay(['a', 'b'], 1), 'player_play', { trackIds: ['a', 'b'], startIndex: 1 }],
    [ipc.playerToggle, 'player_toggle', undefined],
    [ipc.playerNext, 'player_next', undefined],
    [ipc.playerPrev, 'player_prev', undefined],
    [() => ipc.playerSeek(1500.4), 'player_seek', { ms: 1500 }],
    [() => ipc.playerSetVolume(0.3), 'player_set_volume', { v: 0.3 }],
    [ipc.playerState, 'player_state', undefined],
    [() => ipc.playerPlayFiles(['C:/x.flac']), 'player_play_files', { paths: ['C:/x.flac'] }],
    [() => ipc.playerMove(4, 2), 'player_move', { from: 4, to: 2 }],
    [() => ipc.playerRemove(3), 'player_remove', { index: 3 }],
    [() => ipc.libraryAddFolder('C:\\Music'), 'library_add_folder', { path: 'C:\\Music' }],
    [() => ipc.libraryRemoveFolder('C:\\Music'), 'library_remove_folder', { path: 'C:\\Music' }],
    [ipc.libraryFolders, 'library_folders', undefined],
    [ipc.libraryRescan, 'library_rescan', undefined],
    [ipc.libraryAlbums, 'library_albums', undefined],
    [() => ipc.libraryAlbumTracks('local:1'), 'library_album_tracks', { albumId: 'local:1' }],
    [ipc.libraryArtists, 'library_artists', undefined],
    [() => ipc.libraryArtistAlbums('Air'), 'library_artist_albums', { name: 'Air' }],
    [ipc.libraryTrackCount, 'library_track_count', undefined],
    [() => ipc.libraryTracksRange(100, 50), 'library_tracks_range', { offset: 100, limit: 50 }],
    [() => ipc.librarySearch('tear', 20), 'library_search', { q: 'tear', limit: 20 }],
    [ipc.settingsGet, 'settings_get', undefined],
    [() => ipc.settingsSet({ gapless: false }), 'settings_set', { patch: { gapless: false } }],
    [ipc.audioDevices, 'audio_devices', undefined],
    [ipc.startupNotice, 'startup_notice', undefined],
  ];
  for (const [run, command, args] of calls) {
    invoke.mockClear();
    await run();
    expect(invoke).toHaveBeenCalledWith(command, args);
  }
});

it('listens on the fixed event names and unwraps payloads', async () => {
  const seen: unknown[] = [];
  await ipc.onPlayerState((state) => seen.push(state));
  await ipc.onPlayerError((error) => seen.push(error));
  await ipc.onLibraryChanged(() => seen.push('changed'));
  await ipc.onScanProgress((progress) => seen.push(progress));
  const names = listen.mock.calls.map((call) => call[0]);
  expect(names).toEqual(['player://state', 'player://error', 'library://changed', 'library://scan-progress']);
  for (const call of listen.mock.calls) {
    (call[1] as (event: { payload: unknown }) => void)({ payload: call[0] });
  }
  expect(seen).toEqual(['player://state', 'player://error', 'changed', 'library://scan-progress']);
});

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
