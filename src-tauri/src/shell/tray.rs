//! The tray icon (Windows notification area, macOS menu bar).
//!
//! A left click toggles the mini player: at the bottom right on Windows, hanging under the
//! icon on macOS (the icon's place is remembered in `ShellState::last_tray_rect`). The menu
//! (right click; on macOS also ctrl-click) has Open Prism, Play/Pause, Next, Previous and
//! Quit Prism; the transport entries go to whichever player is active (Prism or YouTube
//! Music, `shell::transport`). Its strings come from the main window (`tray_set_labels`), English until then.
//! macOS shows a template icon, drawn in the menu bar's own color.

use serde::Deserialize;
use tauri::image::Image;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalRect, Wry};

use super::mini::{self, anchor_for};
use super::{main_show, press_in, ShellState, TransportButton};

pub const TRAY_ID: &str = "prism";

/// The menu's strings, in the UI language.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayLabels {
    pub open: String,
    pub play_pause: String,
    pub next: String,
    pub previous: String,
    pub quit: String,
}

/// English, until the main window sends the UI language's strings.
impl Default for TrayLabels {
    fn default() -> Self {
        Self {
            open: "Open Prism".into(),
            play_pause: "Play/Pause".into(),
            next: "Next".into(),
            previous: "Previous".into(),
            quit: "Quit Prism".into(),
        }
    }
}

/// The menu's entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    PlayPause,
    Next,
    Previous,
    Quit,
}

impl MenuAction {
    const ALL: [MenuAction; 5] = [
        MenuAction::Open,
        MenuAction::PlayPause,
        MenuAction::Next,
        MenuAction::Previous,
        MenuAction::Quit,
    ];

    fn id(self) -> &'static str {
        match self {
            MenuAction::Open => "tray-open",
            MenuAction::PlayPause => "tray-play-pause",
            MenuAction::Next => "tray-next",
            MenuAction::Previous => "tray-previous",
            MenuAction::Quit => "tray-quit",
        }
    }

    pub fn from_id(id: &str) -> Option<MenuAction> {
        Self::ALL.into_iter().find(|action| action.id() == id)
    }

    /// The transport entries, which go to whichever player is active.
    pub fn button(self) -> Option<TransportButton> {
        match self {
            MenuAction::PlayPause => Some(TransportButton::PlayPause),
            MenuAction::Next => Some(TransportButton::Next),
            MenuAction::Previous => Some(TransportButton::Previous),
            MenuAction::Open | MenuAction::Quit => None,
        }
    }

    fn label(self, labels: &TrayLabels) -> &str {
        match self {
            MenuAction::Open => &labels.open,
            MenuAction::PlayPause => &labels.play_pause,
            MenuAction::Next => &labels.next,
            MenuAction::Previous => &labels.previous,
            MenuAction::Quit => &labels.quit,
        }
    }
}

/// The menu's items, kept so their strings can change.
struct TrayMenu(Vec<(MenuAction, MenuItem<Wry>)>);

/// The macOS menu bar icon: a black silhouette with alpha (`make-icons.mjs`).
#[cfg(target_os = "macos")]
fn icon(_app: &AppHandle) -> Option<Image<'static>> {
    const TEMPLATE: &[u8] = include_bytes!("../../icons/tray-template.png");
    let image = image::load_from_memory_with_format(TEMPLATE, image::ImageFormat::Png)
        .map_err(|err| eprintln!("decoding the tray icon failed: {err}"))
        .ok()?
        .to_rgba8();
    let (width, height) = image.dimensions();
    Some(Image::new_owned(image.into_raw(), width, height))
}

/// Windows: the app icon.
#[cfg(not(target_os = "macos"))]
fn icon(app: &AppHandle) -> Option<Image<'static>> {
    app.default_window_icon()
        .map(|icon| icon.clone().to_owned())
}

/// Creates the tray icon and its menu (at setup).
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let labels = TrayLabels::default();
    let items = MenuAction::ALL
        .into_iter()
        .map(|action| {
            MenuItem::with_id(app, action.id(), action.label(&labels), true, None::<&str>)
                .map(|item| (action, item))
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let item = |action: MenuAction| {
        items
            .iter()
            .find(|(each, _)| *each == action)
            .map(|(_, item)| item)
    };
    let separator = PredefinedMenuItem::separator(app)?;
    let separator_two = PredefinedMenuItem::separator(app)?;
    let (Some(open), Some(play_pause), Some(next), Some(previous), Some(quit)) = (
        item(MenuAction::Open),
        item(MenuAction::PlayPause),
        item(MenuAction::Next),
        item(MenuAction::Previous),
        item(MenuAction::Quit),
    ) else {
        return Ok(());
    };
    let menu = Menu::with_items(
        app,
        &[
            open,
            &separator,
            play_pause,
            next,
            previous,
            &separator_two,
            quit,
        ],
    )?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Prism")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .icon_as_template(cfg!(target_os = "macos"))
        .on_menu_event(on_menu)
        .on_tray_icon_event(on_tray);
    if let Some(icon) = icon(app) {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    app.manage(TrayMenu(items));
    Ok(())
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    let Some(action) = MenuAction::from_id(event.id().as_ref()) else {
        return;
    };
    if let Some(button) = action.button() {
        press_in(app, button, "a tray menu command");
        return;
    }
    match action {
        MenuAction::Open => {
            if let Err(err) = main_show(app) {
                eprintln!("opening Prism failed: {err}");
            }
        }
        MenuAction::PlayPause | MenuAction::Next | MenuAction::Previous => {}
        // `RunEvent::Exit` saves the settings. Marked first, so no window closed on the way
        // out brings the mini player up.
        MenuAction::Quit => {
            if let Some(state) = app.try_state::<ShellState>() {
                state.set_quitting();
            }
            app.exit(0)
        }
    }
}

/// The clicked icon's place in physical px.
fn physical(rect: tauri::Rect) -> PhysicalRect<i32, u32> {
    // The tray reports physical coordinates; the scale only matters for logical ones.
    PhysicalRect {
        position: rect.position.to_physical(1.0),
        size: rect.size.to_physical(1.0),
    }
}

fn on_tray(tray: &TrayIcon, event: TrayIconEvent) {
    let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        rect,
        ..
    } = event
    else {
        return;
    };
    let app = tray.app_handle();
    let menu_bar = cfg!(target_os = "macos");
    let rect = physical(rect);
    if menu_bar {
        if let Some(state) = app.try_state::<ShellState>() {
            state.set_tray_rect(rect);
        }
    }
    if let Err(err) = mini::toggle(app, &anchor_for(Some(rect), menu_bar)) {
        eprintln!("toggling the mini player failed: {err}");
    }
}

/// Puts the menu (and on Windows the thumbnail tooltips) into the UI language.
pub fn set_labels(app: &AppHandle, labels: &TrayLabels) {
    if let Some(menu) = app.try_state::<TrayMenu>() {
        for (action, item) in &menu.0 {
            if let Err(err) = item.set_text(action.label(labels)) {
                eprintln!("relabelling the tray menu failed: {err}");
            }
        }
    }
    #[cfg(target_os = "windows")]
    super::taskbar_win::set_labels(app, labels.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_labels_default_to_english() {
        let labels = TrayLabels::default();
        assert_eq!(labels.open, "Open Prism");
        assert_eq!(labels.play_pause, "Play/Pause");
        assert_eq!(labels.next, "Next");
        assert_eq!(labels.previous, "Previous");
        assert_eq!(labels.quit, "Quit Prism");
    }

    #[test]
    fn tray_labels_come_in_camel_case() {
        let labels: TrayLabels = serde_json::from_value(serde_json::json!({
            "open": "Prism öffnen",
            "playPause": "Wiedergabe/Pause",
            "next": "Weiter",
            "previous": "Zurück",
            "quit": "Prism beenden",
        }))
        .unwrap();
        assert_eq!(labels.play_pause, "Wiedergabe/Pause");
        assert_eq!(MenuAction::Quit.label(&labels), "Prism beenden");
    }

    #[test]
    fn tray_toggle_goes_to_ytm_when_active() {
        use crate::shell::transport::tests::{Call, Rig};
        use crate::ytm::state::YtmAction;

        let rig = Rig::new();
        let press = |action: MenuAction| {
            let button = action.button().expect("a transport entry");
            super::super::press(&rig.transport, button).unwrap();
        };
        press(MenuAction::PlayPause);
        assert_eq!(rig.prism.calls(), vec![Call::Toggle]);
        rig.ytm_plays();
        press(MenuAction::PlayPause);
        press(MenuAction::Next);
        press(MenuAction::Previous);
        assert_eq!(
            rig.ytm.sent(),
            vec![YtmAction::Toggle, YtmAction::Next, YtmAction::Prev]
        );
        assert_eq!(rig.prism.calls(), vec![Call::Toggle]);
        assert_eq!(MenuAction::Open.button(), None);
        assert_eq!(MenuAction::Quit.button(), None);
    }

    #[test]
    fn menu_ids_round_trip() {
        for action in MenuAction::ALL {
            assert_eq!(MenuAction::from_id(action.id()), Some(action));
        }
        assert_eq!(MenuAction::from_id("something-else"), None);
    }
}
