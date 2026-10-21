//! The main window is transparent: the page paints the opaque app card, and the Flow ring
//! hangs half out below the card's bottom edge into the transparent strip under it. Nothing
//! of the system may show in that strip or around the card: no glass material (Mica, macOS
//! vibrancy), no window shadow (config), and on Windows 11 no DWM border, rounded system
//! corners or backdrop. Clicks on the strip beside the ring go through to what is behind the
//! window (`shell::shape`).

use tauri::window::Color;
use tauri::{Manager, State, WindowEvent};

use crate::shell::shape;

/// Whether the main window got a native glass material. Always false now; kept so the
/// frontend can ask, and so a material can return later without an IPC change.
pub struct Vibrancy(pub bool);

/// Turns off what Windows 11's DWM would draw on its own: the 1 px window border, the
/// rounded corners (the card rounds itself) and any system backdrop. Older Windows versions
/// ignore the attributes they do not know.
#[cfg(target_os = "windows")]
pub(crate) fn clear_system_chrome(window: &tauri::WebviewWindow) {
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
    no_classic_frame(window);
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn clear_system_chrome(_window: &tauri::WebviewWindow) {}

/// `DefWindowProc` does not repaint the non-client frame for a `WM_NCACTIVATE` whose lParam
/// is -1.
#[cfg(any(target_os = "windows", test))]
const KEEP_FRAME_UNPAINTED: isize = -1;

/// The lParam a message is passed on with: `WM_NCACTIVATE` gets `KEEP_FRAME_UNPAINTED`,
/// everything else keeps its own.
#[cfg(any(target_os = "windows", test))]
fn passed_lparam(message: u32, lparam: isize) -> isize {
    const WM_NCACTIVATE: u32 = 0x0086;
    if message == WM_NCACTIVATE {
        KEEP_FRAME_UNPAINTED
    } else {
        lparam
    }
}

/// A window with a window region (`shell::shape`) loses DWM's frame rendering, and then
/// `DefWindowProc` paints the classic frame on every activation change (`WM_NCACTIVATE`):
/// light grey bands, 8 px at 100 %, along the window's edges, straight into the window
/// surface (the client area is the whole window). The page covers them only where it is
/// opaque, so they showed as light specks wherever the region reaches into a band over a
/// see-through pixel: the bottom tip of the ring's circle and the card's anti-aliased
/// corners. The subclass passes `WM_NCACTIVATE` on with lParam -1, so the runtime still sees
/// every activation change but nothing paints the frame.
#[cfg(target_os = "windows")]
fn no_classic_frame(window: &tauri::WebviewWindow) {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY;

    const ID: usize = 0x5052_4e46; // "PRNF"

    unsafe extern "system" fn proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if message == WM_NCDESTROY {
            // SAFETY: removing our own subclass from the window that is going away.
            unsafe { RemoveWindowSubclass(hwnd, Some(proc), ID) };
        }
        // SAFETY: the arguments this message arrived with, the lParam of `WM_NCACTIVATE`
        // replaced by -1, which its documentation allows.
        unsafe { DefSubclassProc(hwnd, message, wparam, passed_lparam(message, lparam)) }
    }

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    // SAFETY: `hwnd` is a live window, subclassed on the thread that created it (both
    // windows are set up on the main thread); `proc` lives for the whole program.
    if unsafe { SetWindowSubclass(hwnd.0 as _, Some(proc), ID, 0) } == 0 {
        eprintln!("subclassing the window to keep its frame unpainted failed");
    }
}

/// Clears the main window's background so the area outside the card is see-through.
///
/// The config's `backgroundColor` must stay `#00000000`: on Windows the runtime keeps the
/// color the window was created with and repaints the whole window surface with it on every
/// WM_PAINT (resizes, DPI changes, focus changes), ignoring this call and the alpha. Any
/// non-black color there shows as a tinted band in the transparent strip. `index.html`
/// paints `--bg` itself, so there is still no white first frame.
pub fn setup(app: &tauri::App) {
    if let Some(window) = app.get_webview_window("main") {
        clear_system_chrome(&window);
        if let Err(err) = window.set_background_color(Some(Color(0, 0, 0, 0))) {
            eprintln!("clearing the window background failed: {err}");
        }
        shape::reshape_main(&window);
    }
    app.manage(Vibrancy(false));
}

/// Keeps the main window's click-through shape in step with its size, scale and state
/// (a maximized window has no strip, so no shape). Focus also re-applies it, which restarts
/// the macOS cursor tracker after the window was hidden; moves refresh the tracker's cached
/// position.
pub fn on_main_event(window: &tauri::Window, event: &WindowEvent) {
    let (reshape, moved) = match event {
        WindowEvent::Resized(_) => (true, false),
        WindowEvent::ScaleFactorChanged { .. } => (true, true),
        WindowEvent::Moved(_) => (false, true),
        WindowEvent::Focused(focused) => (cfg!(target_os = "macos") && *focused, false),
        _ => (false, false),
    };
    if !reshape && !moved {
        return;
    }
    let Some(window) = window.get_webview_window(window.label()) else {
        return;
    };
    if moved {
        shape::moved(&window);
    }
    if reshape {
        shape::reshape_main(&window);
    }
}

/// True when the window has a glass material, so the page may paint a translucent frame.
#[tauri::command]
pub fn window_vibrancy(vibrancy: State<'_, Vibrancy>) -> bool {
    vibrancy.0
}

#[cfg(test)]
mod tests {
    use super::{passed_lparam, KEEP_FRAME_UNPAINTED};

    /// Only `WM_NCACTIVATE` loses its lParam, so the classic frame is never painted on an
    /// activation change; every other message passes through unchanged.
    #[test]
    fn activation_changes_keep_the_frame_unpainted() {
        const WM_NCACTIVATE: u32 = 0x0086;
        const WM_NCPAINT: u32 = 0x0085;
        const WM_ACTIVATE: u32 = 0x0006;
        assert_eq!(KEEP_FRAME_UNPAINTED, -1);
        assert_eq!(passed_lparam(WM_NCACTIVATE, 0), -1);
        assert_eq!(passed_lparam(WM_NCACTIVATE, 0x1234), -1);
        assert_eq!(passed_lparam(WM_NCPAINT, 1), 1);
        assert_eq!(passed_lparam(WM_ACTIVATE, 0x1234), 0x1234);
    }

    /// The runtime repaints the window surface with the creation color (see `setup`), so only
    /// transparent black keeps the strip below the card see-through.
    #[test]
    fn main_window_is_created_with_a_transparent_black_background() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let main = &config["app"]["windows"][0];
        assert_eq!(main["label"], "main");
        assert_eq!(main["transparent"], true);
        assert_eq!(main["backgroundColor"], "#00000000");
    }

    /// The HMR socket is a dev-server matter: only the dev CSP allows it.
    #[test]
    fn production_csp_has_no_dev_server_socket() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let security = &config["app"]["security"];
        let prod = security["csp"]["connect-src"].as_str().unwrap();
        assert!(!prod.contains("ws:"), "{prod}");
        let dev = security["devCsp"]["connect-src"].as_str().unwrap();
        assert!(dev.contains("ws://localhost:1441"), "{dev}");
    }

    /// Each window has its own capability; the mini player gets the commands and events
    /// (`core:default`) and nothing that lets a script close, move or resize it.
    #[test]
    fn capabilities_split_main_and_mini() {
        let main: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert_eq!(main["identifier"], "main");
        assert_eq!(main["windows"], serde_json::json!(["main"]));
        let main_permissions = main["permissions"].to_string();
        assert!(main_permissions.contains("core:window:allow-close"));

        let mini: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/mini.json")).unwrap();
        assert_eq!(mini["identifier"], "mini");
        assert_eq!(mini["windows"], serde_json::json!(["mini"]));
        assert_eq!(mini["permissions"], serde_json::json!(["core:default"]));
    }
}
