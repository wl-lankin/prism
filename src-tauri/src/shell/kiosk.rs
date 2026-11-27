//! Kiosk mode: Prism as the only app on a touchscreen (the Raspberry Pi player). The session
//! that starts it (`cage` on tty1, see `scripts/pi/`) sets `PRISM_KIOSK=1`. The main window then
//! fills the screen, and what assumes a desktop around Prism stays off: the mini player, the
//! tray icon, the global shortcut and hiding the window on close.

use std::ffi::OsStr;
use std::sync::OnceLock;

/// The environment variable that turns kiosk mode on (`1`, `true`, `yes` or `on`).
pub const ENV: &str = "PRISM_KIOSK";

/// True when Prism runs as a kiosk. Read once per start.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| is_on(std::env::var_os(ENV).as_deref()))
}

fn is_on(value: Option<&OsStr>) -> bool {
    let Some(value) = value.and_then(OsStr::to_str) else {
        return false;
    };
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Whether the page runs in kiosk mode (no window controls, touch layout).
#[tauri::command]
pub fn window_kiosk() -> bool {
    enabled()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_clear_yes_turns_it_on() {
        for on in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(is_on(Some(OsStr::new(on))), "{on:?}");
        }
        for off in ["", "0", "false", "no", "off", "kiosk"] {
            assert!(!is_on(Some(OsStr::new(off))), "{off:?}");
        }
        assert!(!is_on(None));
    }
}
