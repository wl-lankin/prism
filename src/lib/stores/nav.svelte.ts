import type { Source } from '../types';

export type View =
  | 'home'
  | 'favorites'
  | 'playlists'
  | 'playlist'
  | 'albums'
  | 'artists'
  | 'songs'
  | 'album'
  | 'artist'
  | 'nowplaying'
  | 'radio'
  | 'settings';

export interface Nav {
  view: View;
  /** The album id for 'album', the artist name for 'artist', the playlist id for 'playlist',
   *  the tab (`near`, `search`, `saved`) for 'radio', the section to scroll to (`radio`) for
   *  'settings'. */
  param?: string;
  /** A library view narrowed to one source; none shows every source. */
  source?: Source;
}

/** Where the main window is. M1 shows 'home' as the album grid. */
export const nav: Nav = $state({ view: 'home' });

/** Window-level overlays. `about` is opened from the sidebar logo; the About overlay renders it. */
export const overlays = $state({ about: false });

/** Where `back()` returns to; a short trail is enough. */
const trail: Nav[] = [];
const TRAIL_MAX = 30;

/** Goes to a place. Another source is another place, so `back()` returns to the view it was
 *  narrowed from. */
export function go(view: View, param?: string, source?: Source): void {
  if (nav.view === view && nav.param === param && nav.source === source) return;
  trail.push({ view: nav.view, param: nav.param, source: nav.source });
  if (trail.length > TRAIL_MAX) trail.shift();
  nav.view = view;
  nav.param = param;
  nav.source = source;
}

/** Returns to the previous place, or to the albums when there is none. */
export function back(): void {
  const previous = trail.pop() ?? { view: 'albums' as View };
  nav.view = previous.view;
  nav.param = previous.param;
  nav.source = previous.source;
}
