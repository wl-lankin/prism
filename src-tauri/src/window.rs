//! The main window is transparent: the page paints the opaque app card, and the Flow ring
//! hangs half out below the card's bottom edge into the transparent strip under it. Nothing
//! of the system may show in that strip or around the card: no glass material (Mica, macOS
//! vibrancy), no window shadow (config), and on Windows 11 no DWM border, rounded system
//! corners or backdrop. Clicks on the strip beside the ring go through to what is behind the
//! window (`shell::shape`).

use tauri::window::Color;
use tauri::{Manager, State, WindowEvent};

use crate::shell::shape;

/// Whether the main window got a native glass material. Always false now; kept so the
/// frontend can ask, and so a material can return later without an IPC change.
pub struct Vibrancy(pub bool);

/// Turns off what Windows 11's DWM would draw on its own: the 1 px window border, the
/// rounded corners (the card rounds itself) and any system backdrop. Older Windows versions
/// ignore the attributes they do not know.
#[cfg(target_os = "windows")]
pub(crate) fn clear_system_chrome(window: &tauri::Window) {
    use std::ffi::c_void;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMSBT_NONE, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
        DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let hwnd = hwnd.0 as _;
    let set = |attribute: i32, value: u32| {
        // SAFETY: `hwnd` is the live main window; the value is a 4-byte DWORD as every one of
        // these attributes expects.
        let result = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attribute as u32,
                &value as *const u32 as *const c_void,
                std::mem::size_of::<u32>() as u32,
            )
        };
        if result != 0 {
            eprintln!("DwmSetWindowAttribute({attribute}) failed: {result:#x}");
        }
    };
    set(DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
    set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND as u32);
    set(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_NONE as u32);
    no_classic_frame(window);
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn clear_system_chrome(_window: &tauri::Window) {}

/// `DefWindowProc` does not repaint the non-client frame for a `WM_NCACTIVATE` whose lParam
/// is -1.
#[cfg(any(target_os = "windows", test))]
const KEEP_FRAME_UNPAINTED: isize = -1;

/// The lParam a message is passed on with: `WM_NCACTIVATE` gets `KEEP_FRAME_UNPAINTED`,
/// everything else keeps its own.
#[cfg(any(target_os = "windows", test))]
fn passed_lparam(message: u32, lparam: isize) -> isize {
    const WM_NCACTIVATE: u32 = 0x0086;
    if message == WM_NCACTIVATE {
        KEEP_FRAME_UNPAINTED
    } else {
        lparam
    }
}

/// A window with a window region (`shell::shape`) loses DWM's frame rendering, and then
/// `DefWindowProc` paints the classic frame on every activation change (`WM_NCACTIVATE`):
/// light grey bands, 8 px at 100 %, along the window's edges, straight into the window
/// surface (the client area is the whole window). The page covers them only where it is
/// opaque, so they showed as light specks wherever the region reaches into a band over a
/// see-through pixel: the bottom tip of the ring's circle and the card's anti-aliased
/// corners. The subclass passes `WM_NCACTIVATE` on with lParam -1, so the runtime still sees
/// every activation change but nothing paints the frame.
#[cfg(target_os = "windows")]
fn no_classic_frame(window: &tauri::Window) {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY;

    const ID: usize = 0x5052_4e46; // "PRNF"

    unsafe extern "system" fn proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if message == WM_NCDESTROY {
            // SAFETY: removing our own subclass from the window that is going away.
            unsafe { RemoveWindowSubclass(hwnd, Some(proc), ID) };
        }
        // SAFETY: the arguments this message arrived with, the lParam of `WM_NCACTIVATE`
        // replaced by -1, which its documentation allows.
        unsafe { DefSubclassProc(hwnd, message, wparam, passed_lparam(message, lparam)) }
    }

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    // SAFETY: `hwnd` is a live window, subclassed on the thread that created it (both
    // windows are set up on the main thread); `proc` lives for the whole program.
    if unsafe { SetWindowSubclass(hwnd.0 as _, Some(proc), ID, 0) } == 0 {
        eprintln!("subclassing the window to keep its frame unpainted failed");
    }
}

/// Clears the main window's background so the area outside the card is see-through.
///
/// The config's `backgroundColor` must stay `#00000000`: on Windows the runtime keeps the
/// color the window was created with and repaints the whole window surface with it on every
/// WM_PAINT (resizes, DPI changes, focus changes), ignoring this call and the alpha. Any
/// non-black color there shows as a tinted band in the transparent strip. `index.html`
/// paints `--bg` itself, so there is still no white first frame.
pub fn setup(app: &tauri::App) {
    if let Some(window) = app.get_window(crate::shell::MAIN_LABEL) {
        clear_system_chrome(&window);
        if let Err(err) = window.set_background_color(Some(Color(0, 0, 0, 0))) {
            eprintln!("clearing the window background failed: {err}");
        }
        shape::reshape_main(&window);
    }
    app.manage(Vibrancy(false));
}

/// Keeps the main window's click-through shape in step with its size, scale and state
/// (a maximized window has no strip, so no shape). Focus also re-applies it, which restarts
/// the macOS cursor tracker after the window was hidden; moves refresh the tracker's cached
/// position.
pub fn on_main_event(window: &tauri::Window, event: &WindowEvent) {
    let (reshape, moved) = match event {
        WindowEvent::Resized(_) => (true, false),
        WindowEvent::ScaleFactorChanged { .. } => (true, true),
        WindowEvent::Moved(_) => (false, true),
        WindowEvent::Focused(focused) => (cfg!(target_os = "macos") && *focused, false),
        _ => (false, false),
    };
    if !reshape && !moved {
        return;
    }
    // The window itself: with the YouTube Music view inside, `main` is no longer a
    // webview window.
    if moved {
        shape::moved(window);
    }
    if reshape {
        shape::reshape_main(window);
    }
}

/// True when the window has a glass material, so the page may paint a translucent frame.
#[tauri::command]
pub fn window_vibrancy(vibrancy: State<'_, Vibrancy>) -> bool {
    vibrancy.0
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use super::{passed_lparam, KEEP_FRAME_UNPAINTED};

    /// Only `WM_NCACTIVATE` loses its lParam, so the classic frame is never painted on an
    /// activation change; every other message passes through unchanged.
    #[test]
    fn activation_changes_keep_the_frame_unpainted() {
        const WM_NCACTIVATE: u32 = 0x0086;
        const WM_NCPAINT: u32 = 0x0085;
        const WM_ACTIVATE: u32 = 0x0006;
        assert_eq!(KEEP_FRAME_UNPAINTED, -1);
        assert_eq!(passed_lparam(WM_NCACTIVATE, 0), -1);
        assert_eq!(passed_lparam(WM_NCACTIVATE, 0x1234), -1);
        assert_eq!(passed_lparam(WM_NCPAINT, 1), 1);
        assert_eq!(passed_lparam(WM_ACTIVATE, 0x1234), 0x1234);
    }

    /// The runtime repaints the window surface with the creation color (see `setup`), so only
    /// transparent black keeps the strip below the card see-through.
    #[test]
    fn main_window_is_created_with_a_transparent_black_background() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let main = &config["app"]["windows"][0];
        assert_eq!(main["label"], "main");
        assert_eq!(main["transparent"], true);
        assert_eq!(main["backgroundColor"], "#00000000");
    }

    /// The HMR socket is a dev-server matter: only the dev CSP allows it.
    #[test]
    fn production_csp_has_no_dev_server_socket() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let security = &config["app"]["security"];
        let prod = security["csp"]["connect-src"].as_str().unwrap();
        assert!(!prod.contains("ws:"), "{prod}");
        let dev = security["devCsp"]["connect-src"].as_str().unwrap();
        assert!(dev.contains("ws://localhost:1441"), "{dev}");
    }

    /// Each webview has its own capability; the mini player gets the events (`core:default`)
    /// and its app commands, and nothing that lets a script close, move or resize it. None
    /// is bound by window: the `ytm` child lives in window `main` and must never get main's.
    #[test]
    fn capabilities_split_main_and_mini() {
        for capability in [
            include_str!("../capabilities/default.json"),
            include_str!("../capabilities/mini.json"),
            include_str!("../capabilities/ytm.json"),
        ] {
            let capability = json(capability);
            assert!(
                capability.get("windows").is_none(),
                "{}",
                capability["identifier"]
            );
        }

        let main = json(include_str!("../capabilities/default.json"));
        assert_eq!(main["identifier"], "main");
        assert_eq!(main["webviews"], serde_json::json!(["main"]));
        let main_permissions = main["permissions"].to_string();
        assert!(main_permissions.contains("core:window:allow-close"));

        let mini = json(include_str!("../capabilities/mini.json"));
        assert_eq!(mini["identifier"], "mini");
        assert_eq!(mini["webviews"], serde_json::json!(["mini"]));
        let permissions = mini["permissions"].as_array().unwrap();
        assert_eq!(permissions[0], "core:default");
        for permission in &permissions[1..] {
            let name = permission.as_str().unwrap();
            assert!(name.starts_with("allow-"), "{name}");
        }
    }

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    /// The text between the first `start` and the next `end`.
    fn between<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
        let from = text.find(start).unwrap_or_else(|| panic!("no {start}")) + start.len();
        let len = text[from..].find(end).unwrap_or_else(|| panic!("no {end}"));
        &text[from..from + len]
    }

    fn manifest_commands() -> Vec<String> {
        between(
            include_str!("../build.rs"),
            "const COMMANDS: &[&str] = &[",
            "];",
        )
        .split(',')
        .map(|item| item.trim().trim_matches('"'))
        .filter(|item| !item.is_empty())
        .map(String::from)
        .collect()
    }

    fn handler_commands() -> Vec<String> {
        between(include_str!("lib.rs"), "generate_handler![", "]")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .flat_map(|line| line.split(','))
            .map(|item| item.trim().rsplit("::").next().unwrap())
            .filter(|item| !item.is_empty())
            .map(String::from)
            .collect()
    }

    /// The app commands a capability allows (`allow-<command>` entries).
    fn allowed(capability: &serde_json::Value) -> BTreeSet<String> {
        capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|permission| permission.as_str()?.strip_prefix("allow-"))
            .map(|name| name.replace('-', "_"))
            .collect()
    }

    /// Wrapper name to command name, for every `call('<command>')` wrapper in `ipc.ts`.
    fn ipc_wrappers() -> BTreeMap<String, String> {
        let mut wrappers = BTreeMap::new();
        for part in include_str!("../../src/lib/ipc.ts")
            .split("export const ")
            .skip(1)
        {
            let name: String = part
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let Some(at) = part.find("call<").or_else(|| part.find("call(")) else {
                continue;
            };
            wrappers.insert(name, quoted_after(&part[at..], "('").unwrap().to_string());
        }
        assert!(wrappers.len() > 50, "{wrappers:?}");
        wrappers
    }

    fn quoted_after<'a>(text: &'a str, opener: &str) -> Option<&'a str> {
        let from = text.find(opener)? + opener.len();
        let len = text[from..].find('\'')?;
        Some(&text[from..from + len])
    }

    fn frontend() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../src")
            .canonicalize()
            .unwrap()
    }

    /// The code module behind `spec`, or `None` for a package or an asset (CSS, SVG, JSON).
    fn resolve(dir: &Path, spec: &str) -> Option<PathBuf> {
        if !spec.starts_with('.') {
            return None;
        }
        let base = dir.join(spec).to_string_lossy().into_owned();
        for candidate in ["", ".ts", ".svelte", "/index.ts"] {
            let path = PathBuf::from(format!("{base}{candidate}"));
            if path.is_file() {
                let path = path.canonicalize().unwrap();
                let code = matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("ts" | "js" | "svelte")
                );
                return code.then_some(path);
            }
        }
        panic!("{spec} from {} does not resolve", dir.display());
    }

    fn is_ident(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '$'
    }

    /// `*` stands for a whole module (a default or namespace import, a component).
    const WHOLE: &str = "*";
    const INIT: &str = "init";

    /// A frontend module as the scan sees it: local import names (to module and exported
    /// name), the modules it loads, its top-level declarations, and the code that runs on
    /// load. A Svelte component is all load code.
    #[derive(Clone, Default)]
    struct Module {
        imports: BTreeMap<String, (PathBuf, String)>,
        loads: Vec<PathBuf>,
        decls: BTreeMap<String, String>,
        init: String,
    }

    fn decl_name(line: &str) -> Option<String> {
        let rest = line.strip_prefix("export ").unwrap_or(line);
        let rest = rest.strip_prefix("async ").unwrap_or(rest);
        ["function ", "const ", "let ", "class "]
            .iter()
            .find_map(|keyword| rest.strip_prefix(keyword))
            .map(|rest| {
                rest.chars()
                    .take_while(|c| is_ident(*c))
                    .collect::<String>()
            })
            .filter(|name| !name.is_empty())
    }

    fn parse_module(file: &Path) -> Module {
        let text = std::fs::read_to_string(file).unwrap();
        let dir = file.parent().unwrap();
        let mut module = Module::default();

        let mut offset = 0;
        for line in text.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            if !line.trim_start().starts_with("import ") {
                continue;
            }
            let statement = &text[start..start + text[start..].find(';').unwrap()];
            let statement = statement.trim_start().trim_start_matches("import ");
            if statement.starts_with("type ") {
                continue;
            }
            let (clause, spec) = match statement.rsplit_once(" from ") {
                Some((clause, spec)) => (clause, spec),
                None => ("", statement),
            };
            let Some(target) = resolve(dir, spec.trim().trim_matches('\'')) else {
                continue;
            };
            module.loads.push(target.clone());
            let (default, named) = match clause.split_once('{') {
                Some((default, named)) => (default, named.trim_end().trim_end_matches('}')),
                None => (clause, ""),
            };
            let default = default.trim().trim_end_matches(',').trim();
            if !default.is_empty() {
                assert!(
                    !(default.starts_with("* as ") && target == frontend().join("lib/ipc.ts")),
                    "{}: `import * as` from ipc.ts hides which wrappers it calls; import them by name",
                    file.display()
                );
                let local = default.trim_start_matches("* as ").trim();
                module
                    .imports
                    .insert(local.to_string(), (target.clone(), WHOLE.to_string()));
            }
            for name in named.split(',').map(str::trim) {
                if name.is_empty() || name.starts_with("type ") {
                    continue;
                }
                let (exported, local) = name.split_once(" as ").unwrap_or((name, name));
                module.imports.insert(
                    local.trim().to_string(),
                    (target.clone(), exported.trim().to_string()),
                );
            }
        }

        if file.extension().and_then(|ext| ext.to_str()) == Some("svelte") {
            module.init = text;
            return module;
        }
        let mut current: Option<String> = None;
        let mut skipping = false;
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
                continue;
            }
            assert!(
                !line.starts_with("export {") && !line.starts_with("export *"),
                "{}: the command scan does not follow re-exports: {line}",
                file.display()
            );
            let top_level = line.starts_with(|c: char| !c.is_whitespace() && !"})]".contains(c));
            if top_level {
                skipping = line.starts_with("import ");
                current = decl_name(line);
            }
            if skipping {
                continue;
            }
            let target = match &current {
                Some(name) => module.decls.entry(name.clone()).or_default(),
                None => &mut module.init,
            };
            target.push_str(line);
            target.push('\n');
        }
        module
    }

    /// The app commands one window's page may invoke: the `ipc.ts` wrappers named by the
    /// code it can reach from `main.ts` (following imported names and the top-level
    /// declarations they use, so a shared store counts only with the functions this page
    /// uses), plus any direct `invoke('<command>'`. `other_root` is the other window's
    /// component, which `main.ts` loads only there.
    fn page_commands(other_root: &str) -> BTreeSet<String> {
        let root = frontend();
        let ipc = root.join("lib/ipc.ts");
        let skip = root.join(other_root).canonicalize().unwrap();
        let wrappers = ipc_wrappers();
        let mut modules: BTreeMap<PathBuf, Module> = BTreeMap::new();
        let mut done = BTreeSet::new();
        let mut todo = vec![(root.join("main.ts"), WHOLE.to_string())];
        let mut used = BTreeSet::new();
        while let Some((file, symbol)) = todo.pop() {
            if file == skip || file == ipc || !done.insert((file.clone(), symbol.clone())) {
                continue;
            }
            let module = modules
                .entry(file.clone())
                .or_insert_with(|| parse_module(&file))
                .clone();
            let text = match symbol.as_str() {
                WHOLE => {
                    todo.push((file.clone(), INIT.to_string()));
                    todo.extend(module.decls.keys().map(|name| (file.clone(), name.clone())));
                    continue;
                }
                INIT => {
                    todo.extend(
                        module
                            .loads
                            .iter()
                            .map(|load| (load.clone(), INIT.to_string())),
                    );
                    module.init.clone()
                }
                name => {
                    todo.push((file.clone(), INIT.to_string()));
                    module.decls.get(name).cloned().unwrap_or_default()
                }
            };
            for token in text.split(|c: char| !is_ident(c)) {
                if let Some((target, exported)) = module.imports.get(token) {
                    if *target == ipc {
                        used.extend(wrappers.get(exported).cloned());
                    } else {
                        todo.push((target.clone(), exported.clone()));
                    }
                } else if module.decls.contains_key(token) {
                    todo.push((file.clone(), token.to_string()));
                }
            }
            for (at, _) in text.match_indices("import('") {
                let spec = quoted_after(&text[at..], "('").unwrap();
                todo.extend(resolve(file.parent().unwrap(), spec).map(|m| (m, WHOLE.to_string())));
            }
            for opener in ["invoke(", "invoke<"] {
                for (at, _) in text.match_indices(opener) {
                    let command = quoted_after(&text[at..], "('")
                        .unwrap_or_else(|| panic!("{}: invoke without a literal", file.display()));
                    used.insert(command.to_string());
                }
            }
        }
        used
    }

    fn missing(used: &BTreeSet<String>, allowed: &BTreeSet<String>) -> Vec<String> {
        used.difference(allowed).cloned().collect()
    }

    /// The handler list, the manifest in `build.rs`, the `ipc.ts` wrappers and the
    /// capabilities all name the same commands.
    #[test]
    fn every_command_is_in_the_manifest() {
        let manifest = manifest_commands();
        let handlers = handler_commands();
        let manifest_set: BTreeSet<_> = manifest.iter().cloned().collect();
        let handler_set: BTreeSet<_> = handlers.iter().cloned().collect();
        assert_eq!(manifest_set.len(), manifest.len(), "duplicates in build.rs");
        assert_eq!(handler_set.len(), handlers.len(), "duplicates in lib.rs");
        assert_eq!(manifest_set, handler_set);
        assert!(manifest_set.contains("ytm_report"));

        let wrapped: BTreeSet<_> = ipc_wrappers().into_values().collect();
        assert_eq!(missing(&wrapped, &manifest_set), Vec::<String>::new());
        for capability in [
            include_str!("../capabilities/default.json"),
            include_str!("../capabilities/mini.json"),
            include_str!("../capabilities/ytm.json"),
        ] {
            let allowed = allowed(&json(capability));
            assert_eq!(missing(&allowed, &manifest_set), Vec::<String>::new());
        }
    }

    /// Allowed beyond what the page calls today: two `ipc.ts` wrappers no code calls yet.
    const MAIN_EXTRA: &[&str] = &["player_append", "radio_place"];

    /// The capability allows exactly what the page calls plus `extra`, so a new call fails
    /// here instead of at runtime, and a dropped one does not linger.
    fn assert_capability_matches(capability: &str, mut used: BTreeSet<String>, extra: &[&str]) {
        used.extend(extra.iter().map(|name| name.to_string()));
        let allowed = allowed(&json(capability));
        assert_eq!(
            missing(&used, &allowed),
            Vec::<String>::new(),
            "not allowed"
        );
        assert_eq!(missing(&allowed, &used), Vec::<String>::new(), "not called");
    }

    #[test]
    fn main_capability_covers_the_main_page() {
        let used = page_commands("lib/components/MiniPlayer.svelte");
        assert!(used.contains("settings_set") && used.contains("navidrome_connect"));
        assert!(!used.contains("main_show"));
        assert_capability_matches(
            include_str!("../capabilities/default.json"),
            used,
            MAIN_EXTRA,
        );
    }

    #[test]
    fn mini_capability_covers_the_mini_page() {
        let used = page_commands("App.svelte");
        assert!(used.contains("main_show") && used.contains("player_toggle"));
        assert!(!used.contains("library_search"));
        assert_capability_matches(include_str!("../capabilities/mini.json"), used, &[]);
    }

    /// The mini player plays and shows; it never manages the library, playlists, stations,
    /// the Navidrome account or the YouTube Music view. It reads the settings and the
    /// Navidrome status, and saves repeat and shuffle (they are settings) through
    /// `settings_set`.
    #[test]
    fn mini_has_no_settings_or_navidrome_commands() {
        let mini = allowed(&json(include_str!("../capabilities/mini.json")));
        let forbidden = [
            "audio_devices",
            "library_add_folder",
            "library_remove_folder",
            "library_rescan",
            "playlist_create",
            "playlist_rename",
            "playlist_delete",
            "playlist_add",
            "playlist_remove",
            "playlist_move",
            "radio_search",
            "radio_near",
            "radio_redetect",
            "radio_open_homepage",
            "tray_set_labels",
            "ytm_report",
            "ytm_show",
            "ytm_hide",
            "ytm_sign_in",
            "ytm_sign_out",
        ];
        for name in &mini {
            assert!(
                !name.starts_with("navidrome_") || name == "navidrome_status",
                "{name}"
            );
            assert!(!forbidden.contains(&name.as_str()), "{name}");
        }
    }

    /// The remote YouTube Music page can report and nothing else; no other capability
    /// grants the report or opens itself to a remote origin.
    #[test]
    fn ytm_capability_has_only_ytm_report() {
        let ytm = json(include_str!("../capabilities/ytm.json"));
        assert_eq!(ytm["identifier"], "ytm");
        assert_eq!(ytm["webviews"], serde_json::json!(["ytm"]));
        // Never for an app page, even if the view ever loaded one.
        assert_eq!(ytm["local"], false);
        assert!(ytm.get("windows").is_none());
        assert_eq!(
            ytm["remote"],
            serde_json::json!({ "urls": ["https://music.youtube.com/*"] })
        );
        assert_eq!(ytm["permissions"], serde_json::json!(["allow-ytm-report"]));
        assert!(ytm.get("platforms").is_none());

        for capability in [
            include_str!("../capabilities/default.json"),
            include_str!("../capabilities/mini.json"),
        ] {
            let capability = json(capability);
            assert!(capability.get("remote").is_none());
            assert!(!allowed(&capability).contains("ytm_report"));
        }
    }
}
