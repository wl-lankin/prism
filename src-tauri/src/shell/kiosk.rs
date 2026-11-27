//! Kiosk mode: Prism as the only app on a touchscreen (the Raspberry Pi player). The session
//! that starts it (`cage` on tty1, see `scripts/pi/`) sets `PRISM_KIOSK=1`. The main window then
//! fills the screen, and what assumes a desktop around Prism stays off: the mini player, the
//! tray icon, the global shortcut and hiding the window on close.

use std::ffi::OsStr;
use std::sync::OnceLock;

use serde::Deserialize;

/// The environment variable that turns kiosk mode on (`1`, `true`, `yes` or `on`).
pub const ENV: &str = "PRISM_KIOSK";

/// The page zoom of the kiosk window, e.g. `1.333`: a slow GPU runs the screen at 1280x800
/// (fewer pixels per frame) and the page still lays out at 960x600 points, touch size.
pub const ZOOM_ENV: &str = "PRISM_KIOSK_ZOOM";

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

/// The kiosk window's page zoom from `PRISM_KIOSK_ZOOM`, `None` when unset or outside 0.5 to 3.
pub fn zoom() -> Option<f64> {
    parse_zoom(std::env::var_os(ZOOM_ENV).as_deref())
}

fn parse_zoom(value: Option<&OsStr>) -> Option<f64> {
    let zoom: f64 = value?.to_str()?.trim().parse().ok()?;
    (0.5..=3.0).contains(&zoom).then_some(zoom)
}

/// Turns off the tooltips of a kiosk web view. On a touchscreen a tap leaves the pointer where
/// it was, so the tooltip of the last tapped control stays on screen. WebKitGTK sets its own
/// tooltip text on every hover, so the `query-tooltip` emission is stopped instead.
#[cfg(target_os = "linux")]
pub fn no_tooltips(view: &tauri::Webview) {
    let result = view.with_webview(|platform| {
        use gtk::glib::object::ObjectExt;
        use gtk::prelude::WidgetExt;
        platform
            .inner()
            .connect_query_tooltip(|widget, _, _, _, _| {
                widget.stop_signal_emission_by_name("query-tooltip");
                false
            });
    });
    if let Err(err) = result {
        eprintln!("turning off the kiosk tooltips failed: {err}");
    }
}

/// Whether the page runs in kiosk mode (no window controls, touch layout).
#[tauri::command]
pub fn window_kiosk() -> bool {
    enabled()
}

/// What the kiosk's power buttons ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Power {
    Poweroff,
    Reboot,
}

impl Power {
    fn verb(self) -> &'static str {
        match self {
            Power::Poweroff => "poweroff",
            Power::Reboot => "reboot",
        }
    }
}

/// Switches the device off or restarts it: a kiosk has no desktop with a power menu. Refused
/// outside kiosk mode. Runs `sudo -n /usr/bin/systemctl poweroff` (or `reboot`), which the
/// sudoers rule from `scripts/pi/setup-kiosk.sh` allows the kiosk user without a password;
/// `-n` makes a missing rule fail at once instead of waiting for a password nobody can type.
#[tauri::command]
pub fn kiosk_power(action: Power) -> Result<(), String> {
    if !enabled() {
        return Err("switching the device off is for kiosk mode only".into());
    }
    let output = std::process::Command::new("sudo")
        .args(["-n", "/usr/bin/systemctl", action.verb()])
        .output()
        .map_err(|err| format!("sudo could not start: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let reason: String = String::from_utf8_lossy(&output.stderr)
        .trim()
        .chars()
        .take(200)
        .collect();
    Err(if reason.is_empty() {
        format!("systemctl {} failed", action.verb())
    } else {
        reason
    })
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

    #[test]
    fn power_actions_come_from_the_page_in_lowercase() {
        let action: Power = serde_json::from_str("\"poweroff\"").unwrap();
        assert_eq!(action, Power::Poweroff);
        assert_eq!(action.verb(), "poweroff");
        let action: Power = serde_json::from_str("\"reboot\"").unwrap();
        assert_eq!(action.verb(), "reboot");
        assert!(serde_json::from_str::<Power>("\"halt\"").is_err());
    }

    #[test]
    fn power_is_refused_outside_kiosk_mode() {
        // Tests never run with PRISM_KIOSK set, so nothing is switched off here.
        assert!(!enabled());
        assert!(kiosk_power(Power::Poweroff).is_err());
    }

    #[test]
    fn zoom_takes_a_sane_factor_only() {
        assert_eq!(parse_zoom(Some(OsStr::new("1.333"))), Some(1.333));
        assert_eq!(parse_zoom(Some(OsStr::new(" 2 "))), Some(2.0));
        for bad in ["", "0", "0.2", "4", "big", "NaN", "inf"] {
            assert_eq!(parse_zoom(Some(OsStr::new(bad))), None, "{bad:?}");
        }
        assert_eq!(parse_zoom(None), None);
    }
}
