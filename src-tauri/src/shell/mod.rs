//! The shell around the player: tray, mini player, shortcuts and autostart.

pub mod shape;

use tauri::{AppHandle, Manager};

use crate::settings::Change;
use crate::AppState;

/// Applies a setting that is not the player's. The cover lookup is wired; the mini player and
/// the autostart fill in the rest, until then they only log.
pub fn apply_setting(app: &AppHandle, change: &Change) -> Result<(), String> {
    match change {
        Change::CoverLookup(on) => {
            let state = app
                .try_state::<AppState>()
                .ok_or("the app state is not ready")?;
            state.lookup.set_enabled(*on);
        }
        other => eprintln!("setting changed: {other:?}"),
    }
    Ok(())
}
