//! The main window's glass material: Mica on Windows 11, HUD vibrancy on macOS. Where neither
//! is available (Windows 10, Mica refused) the window keeps its opaque background and the
//! frontend paints the opaque `--bg` itself.

use tauri::window::Color;
use tauri::{Manager, State, WebviewWindow};

/// Whether the main window got a native glass material.
pub struct Vibrancy(pub bool);

#[cfg(target_os = "windows")]
fn apply_material(window: &WebviewWindow) -> bool {
    window_vibrancy::apply_mica(window, Some(true)).is_ok()
}

#[cfg(target_os = "macos")]
fn apply_material(window: &WebviewWindow) -> bool {
    use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
    apply_vibrancy(
        window,
        NSVisualEffectMaterial::HudWindow,
        Some(NSVisualEffectState::Active),
        Some(16.0),
    )
    .is_ok()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn apply_material(_window: &WebviewWindow) -> bool {
    false
}

/// Applies the material to the main window and records whether it took. The window starts
/// with an opaque background (no white flash); it only turns transparent once the material
/// is in place behind it.
pub fn setup(app: &tauri::App) {
    let applied = match app.get_webview_window("main") {
        Some(window) => {
            let applied = apply_material(&window);
            if applied {
                if let Err(err) = window.set_background_color(Some(Color(0, 0, 0, 0))) {
                    eprintln!("clearing the window background failed: {err}");
                }
            }
            applied
        }
        None => false,
    };
    app.manage(Vibrancy(applied));
}

/// True when the window has a glass material, so the page may paint a translucent frame.
#[tauri::command]
pub fn window_vibrancy(vibrancy: State<'_, Vibrancy>) -> bool {
    vibrancy.0
}
