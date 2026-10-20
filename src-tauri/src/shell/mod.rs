//! The shell around the player: tray, mini player, shortcuts and autostart.

pub mod mini;
pub mod open_with;
pub mod shape;
pub mod shortcut;
#[cfg(target_os = "windows")]
pub mod taskbar_win;
pub mod tray;

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, PhysicalRect};

use crate::player::{EventSink, PlayerEvent, PlayerStatus};
use crate::settings::Change;
use crate::AppState;

/// The main window's label.
pub const MAIN_LABEL: &str = "main";

/// The shell's own state (managed).
#[derive(Default)]
pub struct ShellState {
    /// The user closed the main window: on Windows it is minimized and its taskbar button
    /// toggles the mini player; on macOS it is hidden. Cleared by `main_show`.
    pub main_closed: AtomicBool,
    /// The menu bar icon's place from its last click (macOS), where the mini player hangs.
    pub last_tray_rect: Mutex<Option<PhysicalRect<i32, u32>>>,
}

impl ShellState {
    pub fn is_main_closed(&self) -> bool {
        self.main_closed.load(Ordering::Acquire)
    }

    pub fn set_main_closed(&self, closed: bool) {
        self.main_closed.store(closed, Ordering::Release);
    }

    pub fn tray_rect(&self) -> Option<PhysicalRect<i32, u32>> {
        match self.last_tray_rect.lock() {
            Ok(rect) => *rect,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    pub fn set_tray_rect(&self, rect: PhysicalRect<i32, u32>) {
        match self.last_tray_rect.lock() {
            Ok(mut slot) => *slot = Some(rect),
            Err(poisoned) => *poisoned.into_inner() = Some(rect),
        }
    }
}

/// What closing the main window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Windows: minimize and remember it as closed, so the app keeps its single taskbar
    /// button and a click on it toggles the mini player.
    MinimizeAndMarkClosed,
    /// macOS: hide the window; the Dock icon and the menu bar icon stay.
    Hide,
}

pub fn on_main_close(windows: bool) -> CloseAction {
    if windows {
        CloseAction::MinimizeAndMarkClosed
    } else {
        CloseAction::Hide
    }
}

/// What a click on the taskbar button (`SC_RESTORE`) does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreAction {
    /// The window was only minimized: it comes back.
    RestoreMain,
    /// The window was closed: the mini player shows or hides instead.
    ToggleMini,
}

pub fn on_taskbar_restore(main_closed: bool) -> RestoreAction {
    if main_closed {
        RestoreAction::ToggleMini
    } else {
        RestoreAction::RestoreMain
    }
}

/// `WindowEvent::CloseRequested` on the main window (already prevented): Prism keeps running
/// with the mini player, the tray and (Windows) the taskbar button.
pub fn close_main(window: &tauri::Window) {
    // Marked first, so a taskbar click right after the minimize already toggles.
    if let Some(state) = window.app_handle().try_state::<ShellState>() {
        state.set_main_closed(true);
    }
    let done = match on_main_close(cfg!(target_os = "windows")) {
        CloseAction::MinimizeAndMarkClosed => window.minimize(),
        CloseAction::Hide => window.hide(),
    };
    if let Err(err) = done {
        eprintln!("closing the main window failed: {err}");
    }
}

/// What the main window (created hidden) does at the end of setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainAtStart {
    Show,
    /// Started at sign-in: only the tray icon appears, the window counts as closed.
    StayClosed,
}

pub fn main_at_start(autostart: bool) -> MainAtStart {
    if autostart {
        MainAtStart::StayClosed
    } else {
        MainAtStart::Show
    }
}

/// Shows the main window at the end of setup (its first frame is the page's own `--bg`), or,
/// started at sign-in, leaves it hidden and marks it closed.
pub fn start_main(app: &AppHandle, autostart: bool) {
    match main_at_start(autostart) {
        MainAtStart::StayClosed => {
            if let Some(state) = app.try_state::<ShellState>() {
                state.set_main_closed(true);
            }
        }
        MainAtStart::Show => {
            if let Some(window) = app.get_webview_window(MAIN_LABEL) {
                if let Err(err) = window.show() {
                    eprintln!("showing the main window failed: {err}");
                }
            }
        }
    }
}

/// Brings the main window up (shown, restored and focused) and hides the mini player.
pub fn main_show(app: &AppHandle) -> Result<(), String> {
    if let Some(state) = app.try_state::<ShellState>() {
        state.set_main_closed(false);
    }
    if let Err(err) = mini::hide(app) {
        eprintln!("hiding the mini player failed: {err}");
    }
    let window = app
        .get_webview_window(MAIN_LABEL)
        .ok_or("the main window is gone")?;
    let fail = |err: tauri::Error| err.to_string();
    window.show().map_err(fail)?;
    if window.is_minimized().unwrap_or(false) {
        window.unminimize().map_err(fail)?;
    }
    window.set_focus().map_err(fail)
}

/// Whether the play/pause buttons outside the page (the Windows thumbnail toolbar) show pause.
pub fn shows_pause(status: PlayerStatus) -> bool {
    status == PlayerStatus::Playing
}

/// Follows the player for the Windows thumbnail toolbar's play/pause icon; only a change
/// reaches the main thread. Nothing to follow on macOS (the menu says "Play/Pause").
pub fn player_sink(app: AppHandle) -> EventSink {
    const UNKNOWN: u8 = 0;
    let shown = Arc::new(AtomicU8::new(UNKNOWN));
    Arc::new(move |event| {
        let PlayerEvent::State(state) = event else {
            return;
        };
        let pause = shows_pause(state.status);
        let code = if pause { 2 } else { 1 };
        if shown.swap(code, Ordering::AcqRel) == code {
            return;
        }
        #[cfg(target_os = "windows")]
        taskbar_win::set_playing(&app, pause);
        #[cfg(not(target_os = "windows"))]
        let _ = &app;
    })
}

/// Applies a setting that is not the player's: the cover lookup, the mini player's
/// always-on-top, its global shortcut and the start with the system.
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
        // Already registered by `settings_set` before saving; the same shortcut again is a
        // no-op.
        Change::MiniShortcut(accelerator) => shortcut::apply(app, accelerator)?,
        Change::StartWithSystem(on) => shortcut::apply_start_with_system(app, *on)?,
        other => eprintln!("setting changed: {other:?}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_policy_per_platform() {
        // Windows keeps the single taskbar button: the window is minimized, not hidden.
        assert_eq!(on_main_close(true), CloseAction::MinimizeAndMarkClosed);
        // macOS has the Dock icon and the menu bar: the window just goes away.
        assert_eq!(on_main_close(false), CloseAction::Hide);
    }

    #[test]
    fn autostart_keeps_the_main_window_closed() {
        assert_eq!(main_at_start(true), MainAtStart::StayClosed);
        assert_eq!(main_at_start(false), MainAtStart::Show);
    }

    /// Created hidden, so a start at sign-in never flashes the window; setup shows it.
    #[test]
    fn main_window_is_created_hidden() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let main = &config["app"]["windows"][0];
        assert_eq!(main["label"], MAIN_LABEL);
        assert_eq!(main["visible"], false);
    }

    #[test]
    fn taskbar_restore_toggles_only_when_closed() {
        assert_eq!(on_taskbar_restore(true), RestoreAction::ToggleMini);
        // A window the user only minimized comes back as usual.
        assert_eq!(on_taskbar_restore(false), RestoreAction::RestoreMain);
    }

    #[test]
    fn thumb_button_shows_pause_only_while_playing() {
        assert!(shows_pause(PlayerStatus::Playing));
        assert!(!shows_pause(PlayerStatus::Paused));
        assert!(!shows_pause(PlayerStatus::Idle));
        assert!(!shows_pause(PlayerStatus::Loading));
    }

    #[test]
    fn shell_state_starts_open_without_a_tray_rect() {
        let state = ShellState::default();
        assert!(!state.main_closed.load(Ordering::Acquire));
        assert!(state.last_tray_rect.lock().unwrap().is_none());
        state.set_main_closed(true);
        assert!(state.is_main_closed());
        let rect = PhysicalRect {
            position: tauri::PhysicalPosition::new(10, 0),
            size: tauri::PhysicalSize::new(24, 24),
        };
        state.set_tray_rect(rect);
        assert_eq!(state.tray_rect().map(|rect| rect.position.x), Some(10));
    }
}
