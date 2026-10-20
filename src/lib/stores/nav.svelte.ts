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

/** Window-level overlays. `about` is opened from the sidebar logo; Task 12 renders it. */
export const overlays = $state({ about: false });

export function go(view: View, param?: string): void {
  nav.view = view;
  nav.param = param;
}
