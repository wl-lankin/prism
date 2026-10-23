import { LYRICS_PANEL_KEY, NOW_PLAYING_SIDE_KEY, QUEUE_PANEL_KEY } from '../queue';

/** The main window's right-hand slot holds the queue panel or the lyrics panel, never both:
 *  opening one closes the other. Which one is open is remembered per machine (a view
 *  preference, not a setting); without storage the slot simply starts closed. */
export const queuePanel = $state({ open: false });
export const lyricsPanel = $state({ open: false });

function remember(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Storage is blocked: the panel still opens and closes, it is just not remembered.
  }
}

/** Reads the remembered state; call once when the main window starts. */
export function loadQueuePanel(): void {
  try {
    queuePanel.open = localStorage.getItem(QUEUE_PANEL_KEY) === '1';
    lyricsPanel.open = !queuePanel.open && localStorage.getItem(LYRICS_PANEL_KEY) === '1';
  } catch {
    queuePanel.open = false;
    lyricsPanel.open = false;
  }
}

/** Opens or closes the queue panel and remembers it; opening it closes the lyrics panel. */
export function toggleQueuePanel(open = !queuePanel.open): void {
  queuePanel.open = open;
  remember(QUEUE_PANEL_KEY, open ? '1' : '0');
  if (open && lyricsPanel.open) {
    lyricsPanel.open = false;
    remember(LYRICS_PANEL_KEY, '0');
  }
}

/** Opens or closes the lyrics panel and remembers it; opening it closes the queue panel. */
export function toggleLyricsPanel(open = !lyricsPanel.open): void {
  lyricsPanel.open = open;
  remember(LYRICS_PANEL_KEY, open ? '1' : '0');
  if (open && queuePanel.open) {
    queuePanel.open = false;
    remember(QUEUE_PANEL_KEY, '0');
  }
}

export type NowPlayingSide = 'queue' | 'lyrics';

/** What Now Playing shows beside the cover. */
export const nowPlayingSide = $state<{ view: NowPlayingSide }>({ view: 'queue' });

export function loadNowPlayingSide(): void {
  try {
    nowPlayingSide.view = localStorage.getItem(NOW_PLAYING_SIDE_KEY) === 'lyrics' ? 'lyrics' : 'queue';
  } catch {
    nowPlayingSide.view = 'queue';
  }
}

export function setNowPlayingSide(view: NowPlayingSide): void {
  nowPlayingSide.view = view;
  remember(NOW_PLAYING_SIDE_KEY, view);
}
