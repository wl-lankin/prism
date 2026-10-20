export type View =
  | 'home'
  | 'favorites'
  | 'playlists'
  | 'albums'
  | 'artists'
  | 'songs'
  | 'album'
  | 'artist'
  | 'settings';

export interface Nav {
  view: View;
  /** The album id for 'album', the artist name for 'artist'. */
  param?: string;
}

/** Where the main window is. M1 shows 'home' as the album grid. */
export const nav: Nav = $state({ view: 'home' });

/** Window-level overlays. `about` is opened from the sidebar logo; the About overlay renders it. */
export const overlays = $state({ about: false });

/** Where `back()` returns to; a short trail is enough. */
const trail: Nav[] = [];
const TRAIL_MAX = 30;

export function go(view: View, param?: string): void {
  if (nav.view === view && nav.param === param) return;
  trail.push({ view: nav.view, param: nav.param });
  if (trail.length > TRAIL_MAX) trail.shift();
  nav.view = view;
  nav.param = param;
}

/** Returns to the previous place, or to the albums when there is none. */
export function back(): void {
  const previous = trail.pop() ?? { view: 'albums' as View };
  nav.view = previous.view;
  nav.param = previous.param;
}
