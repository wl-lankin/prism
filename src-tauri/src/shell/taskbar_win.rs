//! Windows: the main window's taskbar button and its thumbnail toolbar.
//!
//! Closing the main window hides it (`shell::close_main`), and its taskbar button goes with
//! it; `shell::main_show` brings both back. While the window is open, the thumbnail preview
//! carries previous, play/pause and next (`ITaskbarList3::ThumbBarAddButtons`), added each
//! time the taskbar announces the button (`TaskbarButtonCreated`: at the first show, again
//! after every hide and show, and after Explorer restarts).
//!
//! The macOS counterpart is the Dock: `RunEvent::Reopen` shows the main window (`lib.rs`), and
//! macOS has no thumbnail toolbar.

use std::cell::RefCell;
use std::sync::OnceLock;

use tauri::{AppHandle, Manager, WebviewWindow};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{
    ITaskbarList3, TaskbarList, THBF_ENABLED, THBN_CLICKED, THB_FLAGS, THB_ICON, THB_TOOLTIP,
    THUMBBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::HICON;

use super::tray::TrayLabels;
use crate::AppState;

/// The thumbnail toolbar's buttons, by their command id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbAction {
    Previous,
    PlayPause,
    Next,
}

const ID_PREVIOUS: u32 = 1;
const ID_PLAY_PAUSE: u32 = 2;
const ID_NEXT: u32 = 3;

/// `WM_COMMAND`'s `wParam` from a thumbnail button: `THBN_CLICKED` in the high word, the
/// button's id in the low word.
pub fn thumb_action(wparam: usize) -> Option<ThumbAction> {
    if (wparam >> 16) & 0xFFFF != THBN_CLICKED as usize {
        return None;
    }
    match (wparam & 0xFFFF) as u32 {
        ID_PREVIOUS => Some(ThumbAction::Previous),
        ID_PLAY_PAUSE => Some(ThumbAction::PlayPause),
        ID_NEXT => Some(ThumbAction::Next),
        _ => None,
    }
}

/// The PNG of the entry in an ICO file closest to `size` px: the smallest one at least that
/// big, otherwise the biggest one.
pub fn ico_entry(ico: &[u8], size: u32) -> Option<&[u8]> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(ico.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(ico.get(at..at + 4)?.try_into().ok()?));
    if u16_at(0)? != 0 || u16_at(2)? != 1 {
        return None;
    }
    let count = usize::from(u16_at(4)?);
    let mut best: Option<(u32, usize, usize)> = None;
    for index in 0..count {
        let at = 6 + 16 * index;
        let width = match *ico.get(at)? {
            0 => 256,
            width => u32::from(width),
        };
        let len = usize::try_from(u32_at(at + 8)?).ok()?;
        let offset = usize::try_from(u32_at(at + 12)?).ok()?;
        let better = match best {
            None => true,
            Some((current, ..)) if current >= size => width >= size && width < current,
            Some((current, ..)) => width > current,
        };
        if better {
            best = Some((width, offset, len));
        }
    }
    let (_, offset, len) = best?;
    ico.get(offset..offset.checked_add(len)?)
}

const ICON_PREVIOUS: &[u8] = include_bytes!("../../icons/thumb-prev.ico");
const ICON_PLAY: &[u8] = include_bytes!("../../icons/thumb-play.ico");
const ICON_PAUSE: &[u8] = include_bytes!("../../icons/thumb-pause.ico");
const ICON_NEXT: &[u8] = include_bytes!("../../icons/thumb-next.ico");

/// The thumbnail toolbar, kept on the main thread (the COM object lives in its apartment).
struct ThumbBar {
    taskbar: ITaskbarList3,
    hwnd: HWND,
    previous: HICON,
    play: HICON,
    pause: HICON,
    next: HICON,
    playing: bool,
    labels: TrayLabels,
}

thread_local! {
    static THUMB_BAR: RefCell<Option<ThumbBar>> = const { RefCell::new(None) };
}

static APP: OnceLock<AppHandle> = OnceLock::new();

/// The message the taskbar sends once the window's button exists.
fn taskbar_button_created() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    *MESSAGE.get_or_init(|| {
        let name: Vec<u16> = "TaskbarButtonCreated\0".encode_utf16().collect();
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(name.as_ptr())
        }
    })
}

/// An icon for the thumbnail toolbar at the system's small icon size.
fn load_icon(ico: &[u8]) -> HICON {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, GetSystemMetrics, LR_DEFAULTCOLOR, SM_CXSMICON,
    };

    // SAFETY: a plain metrics query.
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let Some(png) = ico_entry(ico, size as u32) else {
        return HICON::default();
    };
    // SAFETY: `png` is a complete icon image; the call copies it into a new icon.
    let icon = unsafe {
        CreateIconFromResourceEx(
            png.as_ptr(),
            png.len() as u32,
            1,
            0x0003_0000,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
    };
    if icon.is_null() {
        eprintln!("loading a thumbnail button icon failed");
    }
    HICON(icon)
}

fn tip(text: &str) -> [u16; 260] {
    let mut tip = [0u16; 260];
    for (slot, unit) in tip.iter_mut().zip(text.encode_utf16().take(259)) {
        *slot = unit;
    }
    tip
}

impl ThumbBar {
    fn buttons(&self) -> [THUMBBUTTON; 3] {
        let button = |id: u32, icon: HICON, text: &str| THUMBBUTTON {
            dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
            iId: id,
            hIcon: icon,
            szTip: tip(text),
            dwFlags: THBF_ENABLED,
            ..Default::default()
        };
        [
            button(ID_PREVIOUS, self.previous, &self.labels.previous),
            button(
                ID_PLAY_PAUSE,
                if self.playing { self.pause } else { self.play },
                &self.labels.play_pause,
            ),
            button(ID_NEXT, self.next, &self.labels.next),
        ]
    }

    /// Adds the buttons. Every new taskbar button (after a hide and show of the window, or an
    /// Explorer restart) takes them anew; a button can take them only once, so a repeated
    /// call for the same button (it existed already at startup) updates them instead.
    fn add(&self) {
        let buttons = self.buttons();
        // SAFETY: `hwnd` is the live main window; the buttons are copied by the call.
        let added = unsafe { self.taskbar.ThumbBarAddButtons(self.hwnd, &buttons) };
        if added.is_err() {
            self.update();
        }
    }

    fn update(&self) {
        let buttons = self.buttons();
        // SAFETY: as in `add`.
        if let Err(err) = unsafe { self.taskbar.ThumbBarUpdateButtons(self.hwnd, &buttons) } {
            eprintln!("updating the thumbnail buttons failed: {err}");
        }
    }
}

fn with_bar(act: impl FnOnce(&mut ThumbBar)) {
    THUMB_BAR.with(|bar| {
        if let Ok(mut bar) = bar.try_borrow_mut() {
            if let Some(bar) = bar.as_mut() {
                act(bar);
            }
        }
    });
}

/// Subclasses the main window (on the main thread, at setup) and adds the thumbnail buttons
/// if its taskbar button exists already.
pub fn attach(app: &AppHandle, window: &WebviewWindow) {
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;
    use windows_sys::Win32::UI::WindowsAndMessaging::{ChangeWindowMessageFilterEx, MSGFLT_ALLOW};

    let _ = APP.set(app.clone());
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let raw = hwnd.0 as windows_sys::Win32::Foundation::HWND;
    // SAFETY: `raw` is the live main window, subclassed on the thread that created it;
    // `proc` lives for the whole program. The filter lets the taskbar's message through even
    // when Prism runs elevated.
    unsafe {
        ChangeWindowMessageFilterEx(
            raw,
            taskbar_button_created(),
            MSGFLT_ALLOW,
            std::ptr::null_mut(),
        );
        if SetWindowSubclass(raw, Some(proc), SUBCLASS_ID, 0) == 0 {
            eprintln!("subclassing the main window for the taskbar failed");
        }
    }
    // SAFETY: COM is initialised on the main thread (the web view needs it); the object stays
    // on this thread.
    let taskbar: windows::core::Result<ITaskbarList3> = unsafe {
        CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).and_then(
            |list: ITaskbarList3| {
                list.HrInit()?;
                Ok(list)
            },
        )
    };
    let taskbar = match taskbar {
        Ok(taskbar) => taskbar,
        Err(err) => {
            eprintln!("the taskbar list is not available: {err}");
            return;
        }
    };
    let bar = ThumbBar {
        taskbar,
        hwnd: HWND(hwnd.0),
        previous: load_icon(ICON_PREVIOUS),
        play: load_icon(ICON_PLAY),
        pause: load_icon(ICON_PAUSE),
        next: load_icon(ICON_NEXT),
        playing: false,
        labels: TrayLabels::default(),
    };
    bar.add();
    THUMB_BAR.with(|slot| *slot.borrow_mut() = Some(bar));
}

/// The play/pause button shows pause while playing (from the player's events, any thread).
pub fn set_playing(app: &AppHandle, playing: bool) {
    let _ = app.run_on_main_thread(move || {
        with_bar(|bar| {
            if bar.playing != playing {
                bar.playing = playing;
                bar.update();
            }
        })
    });
}

/// The buttons' tooltips, in the UI language.
pub fn set_labels(app: &AppHandle, labels: TrayLabels) {
    let _ = app.run_on_main_thread(move || {
        with_bar(|bar| {
            bar.labels = labels;
            bar.update();
        })
    });
}

const SUBCLASS_ID: usize = 0x5052_5442; // "PRTB"

fn run(action: ThumbAction) {
    let Some(state) = APP.get().and_then(|app| app.try_state::<AppState>()) else {
        return;
    };
    let sent = match action {
        ThumbAction::Previous => state.player.prev(),
        ThumbAction::PlayPause => state.player.toggle(),
        ThumbAction::Next => state.player.next(),
    };
    if let Err(err) = sent {
        eprintln!("a thumbnail button failed: {err}");
    }
}

unsafe extern "system" fn proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_COMMAND, WM_NCDESTROY};

    match message {
        WM_COMMAND => {
            if let Some(action) = thumb_action(wparam) {
                run(action);
                return 0;
            }
        }
        WM_NCDESTROY => {
            // SAFETY: removing our own subclass from the window that is going away.
            unsafe { RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS_ID) };
        }
        other if other == taskbar_button_created() => with_bar(|bar| bar.add()),
        _ => {}
    }
    // SAFETY: the arguments are the ones this message arrived with.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_clicks_map_to_the_player() {
        let click = |id: usize| (THBN_CLICKED as usize) << 16 | id;
        assert_eq!(thumb_action(click(1)), Some(ThumbAction::Previous));
        assert_eq!(thumb_action(click(2)), Some(ThumbAction::PlayPause));
        assert_eq!(thumb_action(click(3)), Some(ThumbAction::Next));
        assert_eq!(thumb_action(click(4)), None);
        // A menu or accelerator command with the same id is not a thumbnail click.
        assert_eq!(thumb_action(2), None);
    }

    #[test]
    fn ico_entry_picks_the_closest_size() {
        let entry = |size: u32| ico_entry(ICON_PLAY, size).map(|png| png.len());
        let sixteen = entry(16).unwrap();
        let thirty_two = entry(32).unwrap();
        assert_ne!(sixteen, thirty_two);
        // 20 and 24 px (125 % and 150 %) scale the 32 px image down.
        assert_eq!(entry(20), Some(thirty_two));
        assert_eq!(entry(24), Some(thirty_two));
        // Bigger than any entry: the biggest.
        assert_eq!(entry(48), Some(thirty_two));
        assert!(ico_entry(ICON_PLAY, 16).unwrap().starts_with(b"\x89PNG"));
        assert_eq!(ico_entry(b"not an icon", 16), None);
    }

    #[test]
    fn thumb_icons_load() {
        for ico in [ICON_PREVIOUS, ICON_PLAY, ICON_PAUSE, ICON_NEXT] {
            assert!(!load_icon(ico).0.is_null());
        }
    }

    #[test]
    fn tooltips_are_cut_to_fit() {
        let long = "x".repeat(400);
        let tip = tip(&long);
        assert_eq!(tip[258], u16::from(b'x'));
        assert_eq!(tip[259], 0);
    }
}
