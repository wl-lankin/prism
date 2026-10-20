//! The main window is transparent: the page paints the opaque app card, and the Flow ring
//! hangs half out below the card's bottom edge into the transparent strip under it. Nothing
//! of the system may show in that strip or around the card: no glass material (Mica, macOS
//! vibrancy), no window shadow (config), and on Windows 11 no DWM border, rounded system
//! corners or backdrop.

use tauri::window::Color;
use tauri::{Manager, State};

/// Whether the main window got a native glass material. Always false now; kept so the
/// frontend can ask, and so a material can return later without an IPC change.
pub struct Vibrancy(pub bool);

/// Turns off what Windows 11's DWM would draw on its own: the 1 px window border, the
/// rounded corners (the card rounds itself) and any system backdrop. Older Windows versions
/// ignore the attributes they do not know.
#[cfg(target_os = "windows")]
fn clear_system_chrome(window: &tauri::WebviewWindow) {
    use std::ffi::c_void;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMSBT_NONE, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
        DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let hwnd = hwnd.0 as _;
    let set = |attribute: i32, value: u32| {
        // SAFETY: `hwnd` is the live main window; the value is a 4-byte DWORD as every one of
        // these attributes expects.
        let result = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attribute as u32,
                &value as *const u32 as *const c_void,
                std::mem::size_of::<u32>() as u32,
            )
        };
        if result != 0 {
            eprintln!("DwmSetWindowAttribute({attribute}) failed: {result:#x}");
        }
    };
    set(DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
    set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND as u32);
    set(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_NONE as u32);
}

#[cfg(not(target_os = "windows"))]
fn clear_system_chrome(_window: &tauri::WebviewWindow) {}

/// Clears the main window's background so the area outside the card is see-through. The
/// window starts with the opaque `--bg` from the config, so there is no white first frame.
pub fn setup(app: &tauri::App) {
    if let Some(window) = app.get_webview_window("main") {
        clear_system_chrome(&window);
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
