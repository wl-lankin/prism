//! The main window is transparent: the page paints the opaque app card, and the Flow ring
//! hangs half out below the card's bottom edge into the transparent strip under it. A native
//! glass material (Mica, macOS vibrancy) would fill that strip too, so none is applied.

use tauri::window::Color;
use tauri::{Manager, State};

/// Whether the main window got a native glass material. Always false now; kept so the
/// frontend can ask, and so a material can return later without an IPC change.
pub struct Vibrancy(pub bool);

/// Clears the main window's background so the area outside the card is see-through. The
/// window starts with the opaque `--bg` from the config, so there is no white first frame.
pub fn setup(app: &tauri::App) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(err) = window.set_background_color(Some(Color(0, 0, 0, 0))) {
            eprintln!("clearing the window background failed: {err}");
        }
    }
    app.manage(Vibrancy(false));
}

/// True when the window has a glass material, so the page may paint a translucent frame.
#[tauri::command]
pub fn window_vibrancy(vibrancy: State<'_, Vibrancy>) -> bool {
    vibrancy.0
}
