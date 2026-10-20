//! The mini player: a small, transparent window docked to a corner of the screen.
//!
//! It never takes focus: it is created unfocusable (Windows `WS_EX_NOACTIVATE`) and shown
//! without activation, so the app the user types in keeps the caret and the keystrokes.
//! Like the main window it is shaped (`shell::shape`): only the card and the Flow ring that
//! hangs half out of its left edge paint and take clicks.
//!
//! The window is taller than the player by `menuRoom` (window-shape.json): the Flow menu
//! opens into that room. While the menu is open the shape is lifted, so the whole window
//! paints and a click beside the menu closes it. The room is above the player when it docks
//! to the bottom of the screen and below it when it hangs under a menu bar icon (macOS).

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::window::Color;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalRect, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use super::shape::{self, Region};

pub const MINI_LABEL: &str = "mini";

/// The gap between the player and the work area's edges (or the menu bar icon), logical px.
const MARGIN: f64 = 12.0;

/// Tells the page where the player sits in the window.
pub const EVENT_LAYOUT: &str = "mini://layout";

/// A rectangle in physical px.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A size in physical px.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeI {
    pub w: i32,
    pub h: i32,
}

/// A position in physical px.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PosI {
    pub x: i32,
    pub y: i32,
}

/// Where the mini player appears.
pub enum Anchor {
    /// The bottom-right corner of the work area (Windows: the primary monitor's).
    WorkAreaBottomRight,
    /// Centred under a rectangle, such as the menu bar icon that was clicked (macOS).
    BelowRect(PhysicalRect<i32, u32>),
}

impl Anchor {
    /// The menu opens away from the screen edge the player docks to.
    fn room_above(&self) -> bool {
        matches!(self, Anchor::WorkAreaBottomRight)
    }
}

/// Where the player of `size` goes in `work_area` for `anchor`, `margin` px from the edges.
/// The result always lies inside the work area (as far as the size allows).
pub fn dock_position(work_area: RectI, size: SizeI, anchor: &Anchor, margin: i32) -> PosI {
    let (x, y) = match anchor {
        Anchor::WorkAreaBottomRight => (
            work_area.x + work_area.w - size.w - margin,
            work_area.y + work_area.h - size.h - margin,
        ),
        Anchor::BelowRect(rect) => {
            let width = i32::try_from(rect.size.width).unwrap_or(i32::MAX);
            let height = i32::try_from(rect.size.height).unwrap_or(i32::MAX);
            (
                rect.position.x + width / 2 - size.w / 2,
                rect.position.y.saturating_add(height) + margin,
            )
        }
    };
    let clamp = |value: i32, start: i32, room: i32, extent: i32| {
        let low = start + margin;
        let high = start + room - extent - margin;
        if high < low {
            low
        } else {
            value.clamp(low, high)
        }
    };
    PosI {
        x: clamp(x, work_area.x, work_area.w, size.w),
        y: clamp(y, work_area.y, work_area.h, size.h),
    }
}

/// The mini window's builder settings, logical px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MiniConfig {
    pub width: f64,
    /// The whole window: the player plus the room for the Flow menu.
    pub height: f64,
    /// The player's part of the window: the card with `padY` above and below.
    pub player_height: f64,
    pub transparent: bool,
    pub decorations: bool,
    pub shadow: bool,
    pub resizable: bool,
    pub skip_taskbar: bool,
    pub focused: bool,
    pub focusable: bool,
    pub visible: bool,
    pub always_on_top: bool,
}

pub fn builder_config(on_top: bool) -> MiniConfig {
    let mini = shape::shapes().mini;
    let player_height = mini.height + 2.0 * mini.pad_y;
    MiniConfig {
        width: mini.pad_left + mini.width + 16.0,
        height: player_height + mini.menu_room,
        player_height,
        transparent: true,
        decorations: false,
        shadow: false,
        resizable: false,
        skip_taskbar: true,
        focused: false,
        focusable: false,
        visible: false,
        always_on_top: on_top,
    }
}

/// Where the player starts in the window, logical px from the top.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct MiniLayout {
    pub top: f64,
}

/// Whether the menu room is above the player (the last anchor's choice).
static ROOM_ABOVE: AtomicBool = AtomicBool::new(true);

/// The player's place in the window for the menu room's side.
pub fn layout_for(room_above: bool) -> MiniLayout {
    MiniLayout {
        top: if room_above {
            shape::shapes().mini.menu_room
        } else {
            0.0
        },
    }
}

pub fn layout() -> MiniLayout {
    layout_for(ROOM_ABOVE.load(Ordering::Acquire))
}

/// The card and the ring, with the player `top` px down in the window.
pub fn region(top: f64) -> Region {
    let mut region = shape::mini_region();
    region.card.y += top;
    region.ring.cy += top;
    region
}

/// Creates the hidden mini window (at setup). It loads the same page as the main window,
/// which mounts the mini player for this label.
pub fn create(app: &AppHandle, on_top: bool) -> tauri::Result<WebviewWindow> {
    let config = builder_config(on_top);
    let window = WebviewWindowBuilder::new(app, MINI_LABEL, WebviewUrl::default())
        .title("Prism")
        .inner_size(config.width, config.height)
        .transparent(config.transparent)
        // Windows keeps the creation color and repaints with it (see `window::setup`).
        .background_color(Color(0, 0, 0, 0))
        .decorations(config.decorations)
        .shadow(config.shadow)
        .resizable(config.resizable)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(config.skip_taskbar)
        .focused(config.focused)
        .focusable(config.focusable)
        .always_on_top(config.always_on_top)
        .visible(config.visible)
        .build()?;
    crate::window::clear_system_chrome(&window);
    shape::apply(&window, Some(&region(layout().top)));
    Ok(window)
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(MINI_LABEL)
}

pub fn is_visible(app: &AppHandle) -> bool {
    window(app).is_some_and(|window| window.is_visible().unwrap_or(false))
}

/// The work area the player docks into, and its scale.
fn work_area(app: &AppHandle, anchor: &Anchor) -> Option<(RectI, f64)> {
    let monitor = match anchor {
        Anchor::BelowRect(rect) => {
            let x = f64::from(rect.position.x) + f64::from(rect.size.width) / 2.0;
            let y = f64::from(rect.position.y) + f64::from(rect.size.height) / 2.0;
            app.monitor_from_point(x, y).ok().flatten()
        }
        Anchor::WorkAreaBottomRight => None,
    }
    .or_else(|| app.primary_monitor().ok().flatten())?;
    let area = monitor.work_area();
    let rect = RectI {
        x: area.position.x,
        y: area.position.y,
        w: i32::try_from(area.size.width).ok()?,
        h: i32::try_from(area.size.height).ok()?,
    };
    Some((rect, monitor.scale_factor()))
}

/// Docks the mini player for `anchor` and shows it without activating it.
pub fn show(app: &AppHandle, anchor: &Anchor) -> tauri::Result<()> {
    let Some(window) = window(app) else {
        return Ok(());
    };
    let config = builder_config(false);
    let room_above = anchor.room_above();
    if ROOM_ABOVE.swap(room_above, Ordering::AcqRel) != room_above {
        let _ = window.emit_to(MINI_LABEL, EVENT_LAYOUT, layout_for(room_above));
    }
    if let Some((area, scale)) = work_area(app, anchor) {
        let px = |v: f64| (v * scale).round() as i32;
        let player = SizeI {
            w: px(config.width),
            h: px(config.player_height),
        };
        let at = dock_position(area, player, anchor, px(MARGIN));
        let room = if room_above {
            px(config.height - config.player_height)
        } else {
            0
        };
        window.set_position(PhysicalPosition::new(at.x, at.y - room))?;
    }
    shape::apply(&window, Some(&region(layout_for(room_above).top)));
    show_inactive(&window);
    Ok(())
}

pub fn hide(app: &AppHandle) -> tauri::Result<()> {
    match window(app) {
        Some(window) => window.hide(),
        None => Ok(()),
    }
}

pub fn toggle(app: &AppHandle, anchor: &Anchor) -> tauri::Result<()> {
    if is_visible(app) {
        hide(app)
    } else {
        show(app, anchor)
    }
}

/// The Flow menu opened or closed: the whole window paints and takes clicks while it is
/// open; the card and ring shape returns when it closes.
pub fn menu(app: &AppHandle, open: bool) {
    if let Some(window) = window(app) {
        if open {
            shape::apply(&window, None);
        } else {
            shape::apply(&window, Some(&region(layout().top)));
        }
    }
}

/// Keeps the shape right after a scale change, and the macOS tracker's position after a move.
pub fn on_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    let Some(window) = window.get_webview_window(window.label()) else {
        return;
    };
    match event {
        tauri::WindowEvent::ScaleFactorChanged { .. } => {
            shape::moved(&window);
            shape::apply(&window, Some(&region(layout().top)));
        }
        tauri::WindowEvent::Moved(_) => shape::moved(&window),
        _ => {}
    }
}

/// Windows: `SW_SHOWNOACTIVATE`, then above the window the user works in, without
/// activation. `HWND_TOP` does not lift a window of a background app over the foreground
/// window (measured: it stayed behind), so the window goes into the topmost band and, unless
/// it is meant to stay on top, straight back out of it: `HWND_NOTOPMOST` leaves it at the top
/// of the normal windows.
#[cfg(target_os = "windows")]
fn show_inactive(window: &WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE, WS_EX_TOPMOST,
    };

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let hwnd = hwnd.0 as _;
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
    // SAFETY: `hwnd` is the live mini window; none of these calls activates it.
    unsafe {
        let on_top = (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST) != 0;
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
        if !on_top {
            SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, flags);
        }
    }
}

/// macOS: `orderFrontRegardless` brings the panel up without making Prism the active app.
/// AppKit wants it on the main thread.
#[cfg(target_os = "macos")]
fn show_inactive(window: &WebviewWindow) {
    let target = window.clone();
    let shown = window.run_on_main_thread(move || {
        let Ok(ns_window) = target.ns_window() else {
            return;
        };
        // SAFETY: `ns_window` is the live NSWindow behind the mini window, used on the main
        // thread.
        #[allow(unused_unsafe)]
        unsafe {
            let ns_window = &*(ns_window as *const objc2_app_kit::NSWindow);
            ns_window.orderFrontRegardless();
        }
    });
    if let Err(err) = shown {
        eprintln!("showing the mini player failed: {err}");
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn show_inactive(window: &WebviewWindow) {
    let _ = window.show();
}

#[cfg(test)]
mod tests {
    use tauri::{PhysicalPosition, PhysicalSize};

    use super::*;

    const SIZE: SizeI = SizeI { w: 392, h: 136 };

    #[test]
    fn docks_inside_work_area() {
        // The taskbar at the bottom.
        let work_area = RectI {
            x: 0,
            y: 0,
            w: 1920,
            h: 1032,
        };
        let anchor = Anchor::WorkAreaBottomRight;
        assert_eq!(
            dock_position(work_area, SIZE, &anchor, 12),
            PosI { x: 1516, y: 884 }
        );
        // The taskbar on the left: still the bottom-right corner.
        let left_bar = RectI {
            x: 48,
            y: 0,
            w: 1872,
            h: 1080,
        };
        assert_eq!(
            dock_position(left_bar, SIZE, &anchor, 12),
            PosI { x: 1516, y: 932 }
        );
        // A second monitor left of the primary one (negative coordinates).
        let left_screen = RectI {
            x: -1920,
            y: 0,
            w: 1920,
            h: 1040,
        };
        assert_eq!(
            dock_position(left_screen, SIZE, &anchor, 12),
            PosI { x: -404, y: 892 }
        );
    }

    #[test]
    fn below_rect_centres_under_the_rect_and_clamps() {
        // macOS: the menu bar takes the top 24 px.
        let work_area = RectI {
            x: 0,
            y: 24,
            w: 1440,
            h: 876,
        };
        let icon = |x: i32| PhysicalRect {
            position: PhysicalPosition::new(x, 0),
            size: PhysicalSize::new(24, 24),
        };
        // Centred under the icon, just below the menu bar.
        assert_eq!(
            dock_position(work_area, SIZE, &Anchor::BelowRect(icon(700)), 12),
            PosI {
                x: 712 - 196,
                y: 36
            }
        );
        // An icon near the right edge: clamped inside the work area.
        assert_eq!(
            dock_position(work_area, SIZE, &Anchor::BelowRect(icon(1420)), 12),
            PosI { x: 1036, y: 36 }
        );
        // Near the left edge.
        assert_eq!(
            dock_position(work_area, SIZE, &Anchor::BelowRect(icon(10)), 12),
            PosI { x: 12, y: 36 }
        );
    }

    #[test]
    fn mini_window_is_unfocusable_and_hidden() {
        let config = builder_config(false);
        assert!(!config.focusable);
        assert!(!config.focused);
        assert!(!config.visible);
        assert!(config.skip_taskbar);
        assert!(config.transparent);
        assert!(!config.decorations);
        assert!(!config.shadow);
        assert!(!config.resizable);
        assert!(!config.always_on_top);
        assert!(builder_config(true).always_on_top);
    }

    #[test]
    fn window_is_the_player_plus_the_menu_room() {
        let mini = shape::shapes().mini;
        let config = builder_config(false);
        // padLeft + width + 16 by height + 2 * padY, plus the room for the Flow menu.
        assert_eq!(config.width, 392.0);
        assert_eq!(config.player_height, 136.0);
        assert_eq!(config.height, 136.0 + mini.menu_room);
        // The Flow menu (three entries, or the genre list) fits into the room.
        assert!(mini.menu_room >= 280.0);
    }

    #[test]
    fn region_follows_the_player_into_the_window() {
        let below = region(0.0);
        assert_eq!(below, shape::mini_region());
        let room = shape::shapes().mini.menu_room;
        let above = region(layout_for(true).top);
        assert_eq!(above.card.y, below.card.y + room);
        assert_eq!(above.ring.cy, below.ring.cy + room);
        assert_eq!(above.card.x, below.card.x);
        assert_eq!(layout_for(false).top, 0.0);
        // The room itself is click-through.
        assert!(!above.contains(200.0, room / 2.0));
        assert!(above.contains(200.0, room + 68.0));
    }
}
