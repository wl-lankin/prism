//! The global shortcut that shows or hides the mini player, and the start with the system.

use std::str::FromStr;
use std::sync::Mutex;

use tauri::{AppHandle, Manager, PhysicalRect};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use super::mini::{self, Anchor};
use super::ShellState;

/// The start of the error when an accelerator cannot be read as a shortcut.
pub const INVALID: &str = "invalid shortcut";
/// The start of the error when the system refuses the shortcut (another app holds it).
pub const TAKEN: &str = "shortcut taken";

/// The argument the system start passes (`tauri_plugin_autostart::init`).
pub const AUTOSTART_ARG: &str = "--autostart";

/// Registers and unregisters global shortcuts; the plugin in the app, a fake in tests.
pub trait ShortcutRegistry {
    type Key: Clone + PartialEq;
    fn parse(&self, accelerator: &str) -> Result<Self::Key, String>;
    fn register(&self, key: &Self::Key) -> Result<(), String>;
    fn unregister(&self, key: &Self::Key) -> Result<(), String>;
}

/// Replaces `current` with the shortcut for `accelerator` and returns it. The new one is
/// registered before the old one goes, so on any failure the previous shortcut stays
/// registered and the error says whether the accelerator is invalid or taken.
pub fn swap<R: ShortcutRegistry>(
    registry: &R,
    current: Option<&R::Key>,
    accelerator: &str,
) -> Result<R::Key, String> {
    let next = registry
        .parse(accelerator)
        .map_err(|err| format!("{INVALID}: {accelerator} ({err})"))?;
    if current == Some(&next) {
        return Ok(next);
    }
    registry
        .register(&next)
        .map_err(|err| format!("{TAKEN}: {accelerator} ({err})"))?;
    if let Some(old) = current {
        if let Err(err) = registry.unregister(old) {
            eprintln!("releasing the previous shortcut failed: {err}");
        }
    }
    Ok(next)
}

/// Reads an accelerator such as `CommandOrControl+Alt+P` as the plugin does.
pub fn parse_accelerator(accelerator: &str) -> Result<Shortcut, String> {
    Shortcut::from_str(accelerator).map_err(|err| err.to_string())
}

/// The global shortcut plugin of a running app.
struct PluginRegistry<'a>(&'a AppHandle);

impl ShortcutRegistry for PluginRegistry<'_> {
    type Key = Shortcut;

    fn parse(&self, accelerator: &str) -> Result<Shortcut, String> {
        parse_accelerator(accelerator)
    }

    fn register(&self, key: &Shortcut) -> Result<(), String> {
        self.0
            .global_shortcut()
            .register(*key)
            .map_err(|err| err.to_string())
    }

    fn unregister(&self, key: &Shortcut) -> Result<(), String> {
        self.0
            .global_shortcut()
            .unregister(*key)
            .map_err(|err| err.to_string())
    }
}

/// The shortcut registered now (managed). The lock also keeps two changes from crossing.
#[derive(Default)]
pub struct CurrentShortcut(Mutex<Option<Shortcut>>);

/// Makes `accelerator` the mini player's global shortcut. On failure the previous one stays
/// registered and the error starts with [`INVALID`] or [`TAKEN`].
pub fn apply(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    let state = app
        .try_state::<CurrentShortcut>()
        .ok_or("the app state is not ready")?;
    let mut current = match state.0.lock() {
        Ok(current) => current,
        Err(poisoned) => poisoned.into_inner(),
    };
    let next = swap(&PluginRegistry(app), current.as_ref(), accelerator)?;
    *current = Some(next);
    Ok(())
}

/// Where the shortcut shows the mini player: on Windows the corner above the taskbar; on
/// macOS the top-right corner under the menu bar (owner decision, 2026-10-21).
pub fn shortcut_anchor(_tray_rect: Option<PhysicalRect<i32, u32>>, macos: bool) -> Anchor {
    if macos {
        Anchor::WorkAreaTopRight
    } else {
        Anchor::WorkAreaBottomRight
    }
}

/// The plugin's handler: a press toggles the mini player (without taking focus).
pub fn on_shortcut(app: &AppHandle, _shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state() != ShortcutState::Pressed {
        return;
    }
    let rect = app
        .try_state::<ShellState>()
        .and_then(|state| state.tray_rect());
    let anchor = shortcut_anchor(rect, cfg!(target_os = "macos"));
    if let Err(err) = mini::toggle(app, &anchor) {
        eprintln!("toggling the mini player failed: {err}");
    }
}

/// Whether the system started Prism at sign-in (`--autostart`, never the program path).
pub fn is_autostart(args: &[String]) -> bool {
    args.iter().skip(1).any(|arg| arg == AUTOSTART_ARG)
}

/// Turns the start at sign-in on or off; the autostart plugin in the app, a fake in tests.
pub trait Launcher {
    fn enable(&self) -> Result<(), String>;
    fn disable(&self) -> Result<(), String>;
    fn is_enabled(&self) -> Result<bool, String>;
}

/// The value `startWithSystem` should have at startup: the system's own state wins (the user
/// may have removed the entry in the Task Manager or the Login Items). `None` when the saved
/// value already matches or the system cannot tell. Only reads, never writes.
pub fn start_with_system_drift(launcher: &dyn Launcher, saved: bool) -> Option<bool> {
    match launcher.is_enabled() {
        Ok(on) if on != saved => Some(on),
        Ok(_) => None,
        Err(err) => {
            eprintln!("reading the start with the system failed: {err}");
            None
        }
    }
}

/// [`start_with_system_drift`] for the running app.
pub fn start_with_system_on_os(app: &AppHandle, saved: bool) -> Option<bool> {
    start_with_system_drift(&PluginLauncher(app), saved)
}

pub fn set_start_with_system(launcher: &dyn Launcher, on: bool) -> Result<(), String> {
    let done = if on {
        launcher.enable()
    } else {
        launcher.disable()
    };
    done.map_err(|err| {
        let what = if on { "turning on" } else { "turning off" };
        format!("{what} the start with the system failed: {err}")
    })
}

/// The autostart plugin of a running app (the HKCU Run key on Windows, a LaunchAgent on
/// macOS).
struct PluginLauncher<'a>(&'a AppHandle);

impl Launcher for PluginLauncher<'_> {
    fn enable(&self) -> Result<(), String> {
        self.0.autolaunch().enable().map_err(|err| err.to_string())
    }

    fn disable(&self) -> Result<(), String> {
        // Nothing to remove when it was never on (the Run value is missing).
        if !self.is_enabled().unwrap_or(true) {
            return Ok(());
        }
        self.0.autolaunch().disable().map_err(|err| err.to_string())
    }

    fn is_enabled(&self) -> Result<bool, String> {
        self.0
            .autolaunch()
            .is_enabled()
            .map_err(|err| err.to_string())
    }
}

/// `Change::StartWithSystem`.
pub fn apply_start_with_system(app: &AppHandle, on: bool) -> Result<(), String> {
    set_start_with_system(&PluginLauncher(app), on)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Keys are accelerator strings; `taken` ones fail to register.
    #[derive(Default)]
    struct FakeRegistry {
        registered: RefCell<Vec<String>>,
        taken: Vec<String>,
    }

    impl ShortcutRegistry for FakeRegistry {
        type Key = String;

        fn parse(&self, accelerator: &str) -> Result<String, String> {
            if accelerator.contains('+') && !accelerator.ends_with('+') {
                Ok(accelerator.to_string())
            } else {
                Err(format!("cannot parse {accelerator}"))
            }
        }

        fn register(&self, key: &String) -> Result<(), String> {
            if self.taken.contains(key) {
                return Err("already registered".into());
            }
            self.registered.borrow_mut().push(key.clone());
            Ok(())
        }

        fn unregister(&self, key: &String) -> Result<(), String> {
            self.registered.borrow_mut().retain(|held| held != key);
            Ok(())
        }
    }

    #[test]
    fn shortcut_rejects_invalid_and_keeps_previous() {
        let registry = FakeRegistry {
            taken: vec!["Ctrl+Alt+T".into()],
            ..FakeRegistry::default()
        };
        let first = swap(&registry, None, "CommandOrControl+Alt+P").unwrap();
        assert_eq!(*registry.registered.borrow(), ["CommandOrControl+Alt+P"]);

        // Not a shortcut: refused as invalid, the old one stays.
        let err = swap(&registry, Some(&first), "P").unwrap_err();
        assert!(err.starts_with(INVALID), "{err}");
        assert_eq!(*registry.registered.borrow(), ["CommandOrControl+Alt+P"]);

        // Held by another app: refused as taken, the old one stays.
        let err = swap(&registry, Some(&first), "Ctrl+Alt+T").unwrap_err();
        assert!(err.starts_with(TAKEN), "{err}");
        assert_eq!(*registry.registered.borrow(), ["CommandOrControl+Alt+P"]);

        // A free one replaces it.
        let next = swap(&registry, Some(&first), "CommandOrControl+Alt+M").unwrap();
        assert_eq!(next, "CommandOrControl+Alt+M");
        assert_eq!(*registry.registered.borrow(), ["CommandOrControl+Alt+M"]);

        // The same one again changes nothing.
        let same = swap(&registry, Some(&next), "CommandOrControl+Alt+M").unwrap();
        assert_eq!(same, next);
        assert_eq!(*registry.registered.borrow(), ["CommandOrControl+Alt+M"]);
    }

    #[test]
    fn the_real_parser_reads_the_default_and_refuses_junk() {
        assert!(parse_accelerator("CommandOrControl+Alt+P").is_ok());
        assert!(parse_accelerator("CommandOrControl+Alt+F12").is_ok());
        assert!(parse_accelerator("Ctrl+Alt+Nope").is_err());
        assert!(parse_accelerator("").is_err());
    }

    #[test]
    fn autostart_flag_detected() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert!(is_autostart(&args(&["prism.exe", "--autostart"])));
        assert!(!is_autostart(&args(&["prism.exe"])));
        assert!(!is_autostart(&args(&["prism.exe", "C:/Music/a.flac"])));
        // The program path itself never counts.
        assert!(!is_autostart(&args(&["--autostart"])));
    }

    #[test]
    fn shortcut_anchor_per_platform() {
        let rect = PhysicalRect {
            position: tauri::PhysicalPosition::new(700, 0),
            size: tauri::PhysicalSize::new(24, 24),
        };
        // Windows: always the corner above the taskbar.
        assert!(matches!(
            shortcut_anchor(Some(rect), false),
            Anchor::WorkAreaBottomRight
        ));
        assert!(matches!(
            shortcut_anchor(None, false),
            Anchor::WorkAreaBottomRight
        ));
        // macOS: always the top-right corner under the menu bar.
        assert!(matches!(
            shortcut_anchor(Some(rect), true),
            Anchor::WorkAreaTopRight
        ));
        assert!(matches!(
            shortcut_anchor(None, true),
            Anchor::WorkAreaTopRight
        ));
    }

    /// Records what the autostart switch was asked to do.
    #[derive(Default)]
    struct FakeLauncher {
        calls: RefCell<Vec<&'static str>>,
        fail: bool,
        /// What the system reports; `None` cannot tell.
        on_os: Option<bool>,
    }

    impl Launcher for FakeLauncher {
        fn enable(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("enable");
            if self.fail {
                Err("denied".into())
            } else {
                Ok(())
            }
        }

        fn disable(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("disable");
            Ok(())
        }

        fn is_enabled(&self) -> Result<bool, String> {
            self.on_os.ok_or_else(|| "unknown".to_string())
        }
    }

    #[test]
    fn startup_trusts_the_system_state_and_only_reads() {
        let on = FakeLauncher {
            on_os: Some(true),
            ..FakeLauncher::default()
        };
        assert_eq!(start_with_system_drift(&on, false), Some(true));
        assert_eq!(start_with_system_drift(&on, true), None);
        let off = FakeLauncher {
            on_os: Some(false),
            ..FakeLauncher::default()
        };
        assert_eq!(start_with_system_drift(&off, true), Some(false));
        // The system cannot tell: the saved value stays.
        let unknown = FakeLauncher::default();
        assert_eq!(start_with_system_drift(&unknown, true), None);
        assert!(on.calls.borrow().is_empty());
        assert!(off.calls.borrow().is_empty());
    }

    #[test]
    fn start_with_system_switches_the_launcher() {
        let launcher = FakeLauncher::default();
        set_start_with_system(&launcher, true).unwrap();
        set_start_with_system(&launcher, false).unwrap();
        assert_eq!(*launcher.calls.borrow(), ["enable", "disable"]);

        let failing = FakeLauncher {
            fail: true,
            ..FakeLauncher::default()
        };
        let err = set_start_with_system(&failing, true).unwrap_err();
        assert!(err.contains("denied"), "{err}");
    }
}
