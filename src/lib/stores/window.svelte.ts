import { getCurrentWindow } from '@tauri-apps/api/window';

/** The main window's state the layout depends on. A maximized window has no transparent
 *  strip below the card, so the Flow ring moves into the transport strip. */
export const windowState = $state({ maximized: false });

/** Follows the window's maximized state; resolves to a function that stops following. */
export async function initWindow(): Promise<() => void> {
  const win = getCurrentWindow();
  const sync = () =>
    win
      .isMaximized()
      .then((value) => (windowState.maximized = value))
      .catch(() => {});
  const stop = await win.onResized(() => void sync());
  await sync();
  return stop;
}
