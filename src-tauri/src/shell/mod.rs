//! The shell around the player: tray, mini player, shortcuts and autostart.

pub mod mini;
pub mod open_with;
pub mod shape;

use tauri::{AppHandle, Manager};

use crate::settings::Change;
use crate::AppState;

/// Applies a setting that is not the player's: the cover lookup and the mini player's
/// always-on-top. The shortcut and the autostart fill in the rest, until then they only log.
pub fn apply_setting(app: &AppHandle, change: &Change) -> Result<(), String> {
    match change {
        Change::CoverLookup(on) => {
            let state = app
                .try_state::<AppState>()
                .ok_or("the app state is not ready")?;
            state.lookup.set_enabled(*on);
        }
        Change::MiniOnTop(on) => {
            if let Some(window) = app.get_webview_window(mini::MINI_LABEL) {
                window
                    .set_always_on_top(*on)
                    .map_err(|err| format!("keeping the mini player on top failed: {err}"))?;
            }
        }
        other => eprintln!("setting changed: {other:?}"),
    }
    Ok(())
}
