import { getCurrentWindow } from '@tauri-apps/api/window';

/** The main window's state the layout depends on. A maximized (or full screen) window has no
 *  transparent strip below the card, so the Flow ring moves into the transport strip. In kiosk
 *  mode (the touchscreen player) the window always fills the screen and has no controls. */
export const windowState = $state({ maximized: false, kiosk: false });

/** Follows the window's maximized and full screen state; resolves to a function that stops
 *  following. */
export async function initWindow(): Promise<() => void> {
  const win = getCurrentWindow();
  const sync = () =>
    Promise.all([win.isMaximized(), win.isFullscreen()])
      // A kiosk always fills the screen: what the compositor reports in between never moves
      // the layout (the Flow ring jumped out of the transport and back on a tap).
      .then(([maximized, fullscreen]) => (windowState.maximized = windowState.kiosk || maximized || fullscreen))
      .catch(() => {});
  const stop = await win.onResized(() => void sync());
  await sync();
  return stop;
}
