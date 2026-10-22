import { QUEUE_PANEL_KEY } from '../queue';

/** The main window's queue panel. Whether it is open is remembered per machine (a view
 *  preference, not a setting); without storage it simply starts closed. */
export const queuePanel = $state({ open: false });

/** Reads the remembered state; call once when the main window starts. */
export function loadQueuePanel(): void {
  try {
    queuePanel.open = localStorage.getItem(QUEUE_PANEL_KEY) === '1';
  } catch {
    queuePanel.open = false;
  }
}

/** Opens or closes the panel and remembers it. */
export function toggleQueuePanel(open = !queuePanel.open): void {
  queuePanel.open = open;
  try {
    localStorage.setItem(QUEUE_PANEL_KEY, open ? '1' : '0');
  } catch {
    // Storage is blocked: the panel still opens and closes, it is just not remembered.
  }
}
