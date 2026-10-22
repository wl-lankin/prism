//! The shell around the player: tray, mini player, shortcuts and autostart.

pub mod mini;
pub mod open_with;
pub mod shape;
pub mod shortcut;
#[cfg(target_os = "windows")]
pub mod taskbar_win;
pub mod transport;
pub mod tray;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, PhysicalRect};

use crate::player::{EventSink, PlayerEvent, PlayerStatus};
use crate::settings::Change;
use crate::ytm::state::YtmStatus;
use crate::AppState;
use transport::{Active, Transport};

/// The main window's label.
pub const MAIN_LABEL: &str = "main";

/// The shell's own state (managed).
#[derive(Default)]
pub struct ShellState {
    /// The user closed the main window: it is hidden, with no taskbar button (Windows) and
    /// only the Dock icon (macOS). Cleared by `main_show`.
    pub main_closed: AtomicBool,
    /// Prism is on its way out (`RunEvent::ExitRequested`): a close of the main window then
    /// brings no mini player up.
    pub quitting: AtomicBool,
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

    pub fn is_quitting(&self) -> bool {
        self.quitting.load(Ordering::Acquire)
    }

    pub fn set_quitting(&self) {
        self.quitting.store(true, Ordering::Release);
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

/// What closing the main window does. On every platform the window is hidden (on Windows its
/// taskbar button goes with it) and Prism keeps running in the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// The user closed the open window: the mini player takes its place (shown without
    /// activation). The close is the user action, so the player still never appears on its own.
    HideAndShowMini,
    /// Prism is quitting, or the window was not open (a start at sign-in leaves it closed):
    /// only hidden, the mini player stays away.
    Hide,
}

pub fn on_main_close(quitting: bool, already_closed: bool) -> CloseAction {
    if quitting || already_closed {
        CloseAction::Hide
    } else {
        CloseAction::HideAndShowMini
    }
}

/// `WindowEvent::CloseRequested` on the main window (already prevented): the window steps
/// aside (`on_main_close`); the tray icon and the global shortcut toggle the mini player, and
/// `main_show` brings the window back.
pub fn close_main(window: &tauri::Window) {
    let app = window.app_handle();
    let action = match app.try_state::<ShellState>() {
        Some(state) => {
            let action = on_main_close(state.is_quitting(), state.is_main_closed());
            state.set_main_closed(true);
            action
        }
        None => CloseAction::Hide,
    };
    // YouTube Music goes with the window and keeps playing.
    if let Some(state) = app.try_state::<AppState>() {
        state.ytm_view.on_main_close();
    }
    if let Err(err) = window.hide() {
        eprintln!("closing the main window failed: {err}");
    }
    if action == CloseAction::HideAndShowMini {
        if let Err(err) = mini::show(app, &mini::home_anchor(app)) {
            eprintln!("showing the mini player failed: {err}");
        }
    }
}

/// What the main window (created hidden) does at the end of setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainAtStart {
    Show,
    /// Started at sign-in: only the tray icon appears, the window counts as closed.
    StayClosed,
}

/// Started at sign-in the window stays closed, unless a startup notice (a quarantined library
/// file) waits for it: that message must not get lost in a hidden window.
pub fn main_at_start(autostart: bool, notice_pending: bool) -> MainAtStart {
    if autostart && !notice_pending {
        MainAtStart::StayClosed
    } else {
        MainAtStart::Show
    }
}

/// Shows the main window at the end of setup (its first frame is the page's own `--bg`), or,
/// started at sign-in, leaves it hidden and marks it closed.
pub fn start_main(app: &AppHandle, autostart: bool) {
    let notice_pending = app
        .try_state::<crate::startup::StartupNotice>()
        .is_some_and(|notice| notice.is_pending());
    match main_at_start(autostart, notice_pending) {
        MainAtStart::StayClosed => {
            if let Some(state) = app.try_state::<ShellState>() {
                state.set_main_closed(true);
            }
            if let Some(state) = app.try_state::<AppState>() {
                state.ytm_view.on_main_close();
            }
        }
        MainAtStart::Show => {
            if let Some(window) = app.get_window(MAIN_LABEL) {
                if let Err(err) = window.show() {
                    eprintln!("showing the main window failed: {err}");
                }
            }
        }
    }
}

/// Brings the main window up and hides the mini player: no longer closed, shown (which gives
/// it its taskbar button back on Windows; the thumbnail buttons follow on
/// `TaskbarButtonCreated`), restored and focused. The way back from the tray menu, the mini
/// player's "Open Prism", a second launch and the Dock.
pub fn main_show(app: &AppHandle) -> Result<(), String> {
    if let Some(state) = app.try_state::<ShellState>() {
        state.set_main_closed(false);
    }
    if let Err(err) = mini::hide(app) {
        eprintln!("hiding the mini player failed: {err}");
    }
    let window = app
        .get_window(MAIN_LABEL)
        .ok_or("the main window is gone")?;
    let fail = |err: tauri::Error| err.to_string();
    window.show().map_err(fail)?;
    if let Some(state) = app.try_state::<AppState>() {
        state.ytm_view.on_main_show();
    }
    if window.is_minimized().unwrap_or(false) {
        window.unminimize().map_err(fail)?;
    }
    window.set_focus().map_err(fail)
}

/// The transport buttons outside the page: the tray menu and the Windows thumbnail toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportButton {
    PlayPause,
    Next,
    Previous,
}

/// Presses `button` on whichever player is active.
pub fn press(transport: &Transport, button: TransportButton) -> Result<(), String> {
    match button {
        TransportButton::PlayPause => transport.toggle(),
        TransportButton::Next => transport.next(),
        TransportButton::Previous => transport.prev(),
    }
}

/// [`press`] through the app state; `source` names the entry point in the log.
pub fn press_in(app: &AppHandle, button: TransportButton, source: &str) {
    if let Some(state) = app.try_state::<AppState>() {
        if let Err(err) = press(&state.transport, button) {
            eprintln!("{source} failed: {err}");
        }
    }
}

/// Whether the play/pause button outside the page (the Windows thumbnail toolbar) shows
/// pause: while the active player plays.
pub fn shows_pause(active: Active, prism: PlayerStatus, ytm: YtmStatus) -> bool {
    match active {
        Active::Prism => prism == PlayerStatus::Playing,
        Active::Ytm => ytm == YtmStatus::Playing,
    }
}

/// What the play/pause icon shows. Each update answers the new icon (true: pause) when it
/// changed, `None` otherwise.
#[derive(Debug)]
pub struct PlayIcon {
    active: Active,
    prism: PlayerStatus,
    ytm: YtmStatus,
    shown: Option<bool>,
}

impl Default for PlayIcon {
    fn default() -> Self {
        PlayIcon {
            active: Active::Prism,
            prism: PlayerStatus::Idle,
            ytm: YtmStatus::Idle,
            shown: None,
        }
    }
}

impl PlayIcon {
    pub fn prism(&mut self, status: PlayerStatus) -> Option<bool> {
        self.prism = status;
        self.settle()
    }

    pub fn ytm(&mut self, status: YtmStatus) -> Option<bool> {
        self.ytm = status;
        self.settle()
    }

    pub fn active(&mut self, active: Active) -> Option<bool> {
        self.active = active;
        self.settle()
    }

    fn settle(&mut self) -> Option<bool> {
        let pause = shows_pause(self.active, self.prism, self.ytm);
        (self.shown.replace(pause) != Some(pause)).then_some(pause)
    }
}

/// Follows both players and the active one for the Windows thumbnail toolbar's play/pause
/// icon; only a change reaches the main thread. Nothing to show on macOS (no thumbnail
/// toolbar, and the tray menu says "Play/Pause").
pub struct PlayButton {
    icon: Mutex<PlayIcon>,
    app: AppHandle,
}

impl PlayButton {
    pub fn new(app: AppHandle) -> Arc<PlayButton> {
        Arc::new(PlayButton {
            icon: Mutex::new(PlayIcon::default()),
            app,
        })
    }

    fn update(&self, change: impl FnOnce(&mut PlayIcon) -> Option<bool>) {
        let changed = match self.icon.lock() {
            Ok(mut icon) => change(&mut icon),
            Err(poisoned) => change(&mut poisoned.into_inner()),
        };
        #[cfg(target_os = "windows")]
        if let Some(pause) = changed {
            taskbar_win::set_playing(&self.app, pause);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (changed, &self.app);
    }

    pub fn on_ytm(&self, status: YtmStatus) {
        self.update(|icon| icon.ytm(status));
    }

    pub fn on_active(&self, active: Active) {
        self.update(|icon| icon.active(active));
    }
}

/// Prism's side of the [`PlayButton`], for the player's event fan-out.
pub fn player_sink(button: Arc<PlayButton>) -> EventSink {
    Arc::new(move |event| {
        if let PlayerEvent::State(state) = event {
            button.update(|icon| icon.prism(state.status));
        }
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
                mini::apply_panel(&window.as_ref().window(), *on);
            }
        }
        // Applied in settings_set, outside the settings lock (before saving, so a refusal
        // reaches the UI and nothing is saved). Nothing to do here.
        Change::MiniShortcut(_) | Change::StartWithSystem(_) => {}
        other => eprintln!("setting changed: {other:?}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_the_open_window_brings_the_mini_player() {
        assert_eq!(on_main_close(false, false), CloseAction::HideAndShowMini);
    }

    #[test]
    fn no_mini_player_when_quitting_or_never_open() {
        // Tray "Quit" or app.exit: the windows go, nothing new appears.
        assert_eq!(on_main_close(true, false), CloseAction::Hide);
        // Started at sign-in the window was never open (already marked closed).
        assert_eq!(on_main_close(false, true), CloseAction::Hide);
        assert_eq!(on_main_close(true, true), CloseAction::Hide);
    }

    #[test]
    fn quitting_is_remembered() {
        let state = ShellState::default();
        assert!(!state.is_quitting());
        state.set_quitting();
        assert!(state.is_quitting());
    }

    #[test]
    fn autostart_keeps_the_main_window_closed() {
        assert_eq!(main_at_start(true, false), MainAtStart::StayClosed);
        assert_eq!(main_at_start(false, false), MainAtStart::Show);
        // A quarantine notice must reach a visible window, also at sign-in.
        assert_eq!(main_at_start(true, true), MainAtStart::Show);
        assert_eq!(main_at_start(false, true), MainAtStart::Show);
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

    /// With the YouTube Music view inside, `main` holds two webviews and is no longer a
    /// webview window: a lookup of it as one finds nothing. It is looked up as a window.
    #[test]
    fn main_is_never_looked_up_as_a_webview_window() {
        fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    sources(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        sources(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        assert!(files.len() > 20);
        let call = format!("get_webview_{}(", "window");
        let mut found = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).unwrap();
            for (at, _) in text.match_indices(&call) {
                let rest = &text[at + call.len()..];
                let argument = &rest[..rest.find(')').unwrap_or(rest.len())];
                let main = argument.contains("MAIN_LABEL")
                    || argument.contains("\"main\"")
                    || argument.contains(".label()");
                if main {
                    found.push(format!("{}: {argument}", file.display()));
                }
            }
        }
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn thumb_button_shows_pause_only_while_playing() {
        let prism = |status| shows_pause(Active::Prism, status, YtmStatus::Playing);
        assert!(prism(PlayerStatus::Playing));
        assert!(!prism(PlayerStatus::Paused));
        assert!(!prism(PlayerStatus::Idle));
        assert!(!prism(PlayerStatus::Loading));
        let ytm = |status| shows_pause(Active::Ytm, PlayerStatus::Playing, status);
        assert!(ytm(YtmStatus::Playing));
        assert!(!ytm(YtmStatus::Paused));
        assert!(!ytm(YtmStatus::Idle));
        assert!(!ytm(YtmStatus::Unavailable));
    }

    #[test]
    fn thumb_buttons_follow_the_active_player() {
        use crate::shell::transport::tests::{Call, Rig};
        use crate::ytm::state::YtmAction;

        // The buttons press whichever player is active.
        let rig = Rig::new();
        for button in [
            TransportButton::Previous,
            TransportButton::PlayPause,
            TransportButton::Next,
        ] {
            press(&rig.transport, button).unwrap();
        }
        assert_eq!(
            rig.prism.calls(),
            vec![Call::Prev, Call::Toggle, Call::Next]
        );
        rig.ytm_plays();
        for button in [
            TransportButton::Previous,
            TransportButton::PlayPause,
            TransportButton::Next,
        ] {
            press(&rig.transport, button).unwrap();
        }
        assert_eq!(
            rig.ytm.sent(),
            vec![YtmAction::Prev, YtmAction::Toggle, YtmAction::Next]
        );
        assert_eq!(rig.prism.calls().len(), 3, "nothing more went to Prism");

        // The play/pause icon follows the active player, and only a change goes out.
        let mut icon = PlayIcon::default();
        assert_eq!(icon.prism(PlayerStatus::Playing), Some(true));
        assert_eq!(icon.prism(PlayerStatus::Playing), None);
        assert_eq!(icon.ytm(YtmStatus::Paused), None, "Prism is active");
        assert_eq!(icon.prism(PlayerStatus::Paused), Some(false));
        assert_eq!(icon.ytm(YtmStatus::Playing), None, "still Prism's icon");
        assert_eq!(icon.active(Active::Ytm), Some(true));
        assert_eq!(
            icon.prism(PlayerStatus::Playing),
            None,
            "YouTube Music's icon now"
        );
        assert_eq!(icon.ytm(YtmStatus::Paused), Some(false));
        assert_eq!(icon.active(Active::Prism), Some(true));
        assert_eq!(icon.active(Active::Prism), None);
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
