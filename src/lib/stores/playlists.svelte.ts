import { t } from '../i18n';
import {
  navidromePlaylists,
  onPlaylistsChanged,
  playlistAdd,
  playlistCreate,
  playlistRename,
  playlistsList,
} from '../ipc';
import type { Playlist, RemotePlaylist } from '../types';
import { notify } from './notice.svelte';

/** Every playlist, newest change first, as `playlists_list` returns them. */
class PlaylistsStore {
  /** null until the first load. Replaced, never mutated. */
  list = $state.raw<Playlist[] | null>(null);
  /** The Navidrome server's playlists (read-only), most recently changed first. null until
   *  the first load, empty without an account. Replaced, never mutated. */
  remote = $state.raw<RemotePlaylist[] | null>(null);
  /** Bumped on every reload, so an open playlist refetches its entries. */
  version = $state(0);
  /** A playlist just created from the Playlists view: its view opens with the title in edit. */
  renaming = $state<number | null>(null);
}

export const playlists = new PlaylistsStore();

let loading: Promise<void> | null = null;
let again = false;

/** The server's playlists; a failure keeps the last list (or none) so the Prism playlists
 *  still load. */
async function loadRemote(): Promise<RemotePlaylist[]> {
  try {
    return (await navidromePlaylists()) ?? [];
  } catch (err) {
    console.error('loading the Navidrome playlists failed', err);
    return playlists.remote ?? [];
  }
}

/** Reloads the Prism and the Navidrome playlists. `playlists://changed` covers both (a sync
 *  that changed the server's playlists sends it too). Calls during a reload queue one more. */
export function reloadPlaylists(): Promise<void> {
  if (loading) {
    again = true;
    return loading;
  }
  loading = (async () => {
    try {
      do {
        again = false;
        const [list, remote] = await Promise.all([playlistsList(), loadRemote()]);
        playlists.list = list ?? [];
        playlists.remote = remote;
        playlists.version++;
      } while (again);
    } catch (err) {
      console.error('loading the playlists failed', err);
      if (playlists.list === null) notify(t('playlists.loadFailed', { message: String(err) }));
    } finally {
      loading = null;
    }
  })();
  return loading;
}

/** Loads the playlists, then follows `playlists://changed`. Resolves with the cleanup. */
export async function initPlaylists(): Promise<() => void> {
  let stop: (() => void) | undefined;
  try {
    stop = await onPlaylistsChanged(() => void reloadPlaylists());
  } catch (err) {
    console.error('following the playlists failed', err);
  }
  await reloadPlaylists();
  return () => stop?.();
}

/** "New playlist", or "New playlist 2", 3, ... when that name is taken. */
export function suggestName(): string {
  const taken = new Set((playlists.list ?? []).map((p) => p.name.toLowerCase()));
  const base = t('playlists.defaultName');
  if (!taken.has(base.toLowerCase())) return base;
  for (let n = 2; ; n++) {
    const name = t('playlists.defaultNameN', { n });
    if (!taken.has(name.toLowerCase())) return name;
  }
}

/** Creates a playlist and puts it at the top of the list at once. Null (with a notice) on failure. */
export async function createPlaylist(name: string): Promise<Playlist | null> {
  try {
    const created = await playlistCreate(name.trim());
    playlists.list = [created, ...(playlists.list ?? []).filter((p) => p.id !== created.id)];
    return created;
  } catch (err) {
    notify(t('playlists.createFailed', { message: String(err) }));
    return null;
  }
}

/** Appends tracks and confirms it above the ring. */
export async function addToPlaylist(playlist: Pick<Playlist, 'id' | 'name'>, trackIds: string[]): Promise<boolean> {
  if (!trackIds.length) return false;
  try {
    await playlistAdd(playlist.id, trackIds);
    notify(t('playlists.added', { name: playlist.name }));
    return true;
  } catch (err) {
    notify(t('playlists.addFailed', { message: String(err) }));
    return false;
  }
}

/** Creates a playlist named `name`, then appends the tracks to it. */
export async function createWithTracks(name: string, trackIds: string[]): Promise<Playlist | null> {
  const created = await createPlaylist(name);
  if (created) await addToPlaylist(created, trackIds);
  return created;
}

/** Shows the new name at once; a failure puts the old one back with a notice. */
export async function renamePlaylist(id: number, name: string): Promise<void> {
  const clean = name.trim();
  const before = playlists.list?.find((p) => p.id === id);
  if (!clean || !before || before.name === clean) return;
  const patch = (value: string) =>
    (playlists.list = (playlists.list ?? []).map((p) => (p.id === id ? { ...p, name: value } : p)));
  patch(clean);
  try {
    await playlistRename(id, clean);
  } catch (err) {
    patch(before.name);
    notify(t('playlist.renameFailed', { message: String(err) }));
  }
}
