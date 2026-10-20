//! The shell around the player: tray, mini player, shortcuts and autostart.

pub mod shape;

use tauri::AppHandle;

use crate::settings::Change;

/// Applies a setting that is not the player's. It only logs for now; the cover lookup, the
/// mini player and the autostart fill it in.
pub fn apply_setting(_app: &AppHandle, change: &Change) -> Result<(), String> {
    eprintln!("setting changed: {change:?}");
    Ok(())
}
