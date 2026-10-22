//! Debug builds only: `PRISM_YTM_URL=http://127.0.0.1:<port>/` points the YouTube Music view at
//! the fake page (`ytm::testpage::serve_for_ui`), so reports, exclusive playback and overlays
//! run without Google. The variable is read once, at startup. Release builds never read it,
//! never let a `127.0.0.1` page report, and never get its capability.

use std::sync::OnceLock;

use tauri::Url;

pub const DEV_URL_VAR: &str = "PRISM_YTM_URL";
/// The capability the fake page gets at runtime (debug builds, override active).
pub const DEV_CAPABILITY: &str = "ytm-dev";

/// `http://127.0.0.1:<port>/` and nothing else: no other host or scheme, no path, query,
/// fragment or user, an explicit port other than 0 and 80.
pub fn parse_dev_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    // `Url` drops a default port, so 80 has none here.
    let port = url.port().filter(|port| *port != 0)?;
    let exact = format!("http://127.0.0.1:{port}/");
    (value == exact && url.as_str() == exact).then_some(url)
}

/// The override as a build with `debug` set reads it: `read` runs only when `debug` is true.
pub fn override_from(debug: bool, read: impl FnOnce() -> Option<String>) -> Option<Url> {
    if !debug {
        return None;
    }
    let value = read()?;
    let page = parse_dev_url(&value);
    if page.is_none() {
        eprintln!("{DEV_URL_VAR} ignored: only http://127.0.0.1:<port>/ is accepted");
    }
    page
}

#[cfg(debug_assertions)]
fn read_env() -> Option<String> {
    std::env::var(DEV_URL_VAR).ok()
}

#[cfg(not(debug_assertions))]
fn read_env() -> Option<String> {
    None
}

/// The fake page the view loads instead of YouTube Music, if the override is active.
pub fn page() -> Option<&'static Url> {
    static PAGE: OnceLock<Option<Url>> = OnceLock::new();
    PAGE.get_or_init(|| override_from(cfg!(debug_assertions), read_env))
        .as_ref()
}

/// The bridge's `ALLOW_DEV_HOST` for a view that loads `page`.
pub fn bridge_flag(page: Option<&Url>) -> bool {
    cfg!(debug_assertions) && page.is_some()
}

/// The fake page's IPC: `ytm_report` only, for the `ytm` view, on exactly its origin.
#[cfg(debug_assertions)]
pub fn capability(page: &Url) -> tauri::ipc::CapabilityBuilder {
    tauri::ipc::CapabilityBuilder::new(DEV_CAPABILITY)
        .local(false)
        .webview(super::VIEW_LABEL)
        .remote(format!("{page}*"))
        .permission("allow-ytm-report")
}

/// At startup, before any view exists: with the override, a page on `127.0.0.1` may report
/// and the fake page gets its capability.
#[cfg(debug_assertions)]
pub fn activate<R: tauri::Runtime, M: tauri::Manager<R>>(app: &M) {
    let Some(page) = page() else {
        return;
    };
    super::allow_dev_host(true);
    if let Err(err) = app.add_capability(capability(page)) {
        eprintln!("the fake YouTube Music page gets no IPC: {err}");
    }
    eprintln!("YouTube Music view: fake page at {page}");
}

#[cfg(not(debug_assertions))]
pub fn activate<R: tauri::Runtime, M: tauri::Manager<R>>(_app: &M) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    const PAGE: &str = "http://127.0.0.1:4100/";

    #[test]
    fn dev_url_override_accepts_loopback_only() {
        for good in [PAGE, "http://127.0.0.1:1/", "http://127.0.0.1:65535/"] {
            let url = parse_dev_url(good).unwrap_or_else(|| panic!("{good}"));
            assert_eq!(url.as_str(), good);
        }
        for bad in [
            "",
            "http://127.0.0.1:4100",
            "https://127.0.0.1:4100/",
            "http://localhost:4100/",
            "http://127.0.0.2:4100/",
            "http://127.1:4100/",
            "http://0x7f000001:4100/",
            "http://[::1]:4100/",
            "http://0.0.0.0:4100/",
            "http://192.168.2.5:4100/",
            "http://127.0.0.1/",
            "http://127.0.0.1:80/",
            "http://127.0.0.1:0/",
            "http://127.0.0.1:04100/",
            "http://127.0.0.1:99999/",
            "http://127.0.0.1:4100/x",
            "http://127.0.0.1:4100/?a=1",
            "http://127.0.0.1:4100/#a",
            "http://demo@127.0.0.1:4100/",
            " http://127.0.0.1:4100/",
            "http://127.0.0.1:4100/ ",
            "HTTP://127.0.0.1:4100/",
            "https://music.youtube.com/",
            "file:///tmp/index.html",
        ] {
            assert!(parse_dev_url(bad).is_none(), "{bad:?}");
        }

        // What a debug build makes of the variable.
        let read = |value: &'static str| override_from(true, || Some(value.to_string()));
        assert_eq!(read(PAGE).map(String::from), Some(PAGE.to_string()));
        assert!(read("http://localhost:4100/").is_none());
        assert!(override_from(true, || None).is_none());
    }

    #[test]
    fn release_never_reads_the_override() {
        let asked = AtomicBool::new(false);
        let release = override_from(false, || {
            asked.store(true, Ordering::SeqCst);
            Some(PAGE.to_string())
        });
        assert!(release.is_none());
        assert!(
            !asked.load(Ordering::SeqCst),
            "a release build read the variable"
        );

        #[cfg(not(debug_assertions))]
        {
            assert!(read_env().is_none());
            assert!(page().is_none());
            assert!(!crate::ytm::dev_host());
        }

        // The startup read goes through the build's flag, and the variable is read in one
        // debug-only place.
        let source = include_str!("dev.rs");
        let code = &source[..source.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(code.contains("override_from(cfg!(debug_assertions), read_env)"));
        assert_eq!(code.matches("std::env::var").count(), 1);
        assert!(code.contains(
            "#[cfg(debug_assertions)]\nfn read_env() -> Option<String> {\n    std::env::var(DEV_URL_VAR).ok()"
        ));
    }

    #[test]
    fn bridge_prefix_matches_the_build() {
        let page = parse_dev_url(PAGE).unwrap();
        assert!(!bridge_flag(None));
        assert_eq!(bridge_flag(Some(&page)), cfg!(debug_assertions));
        assert!(crate::ytm::bridge_script(bridge_flag(None))
            .starts_with("{ const ALLOW_DEV_HOST = false;\n"));
        let prefix = format!("{{ const ALLOW_DEV_HOST = {};\n", cfg!(debug_assertions));
        assert!(crate::ytm::bridge_script(bridge_flag(Some(&page))).starts_with(&prefix));
        // The view's script and its page come from the same override.
        let view = include_str!("view.rs");
        assert!(view.contains("let dev_page = super::dev::page();"));
        assert!(view.contains("super::bridge_script(super::dev::bridge_flag(dev_page))"));
        assert!(!view.contains("bridge_script(super::dev_host())"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_capability_is_the_page_origin_only() {
        use tauri::ipc::RuntimeCapability;
        use tauri::utils::acl::capability::CapabilityFile;
        use tauri::utils::acl::RemoteUrlPattern;

        let page = parse_dev_url(PAGE).unwrap();
        let CapabilityFile::Capability(capability) = capability(&page).build() else {
            panic!("one capability");
        };
        let json = serde_json::to_value(&capability).unwrap();
        assert_eq!(json["identifier"], DEV_CAPABILITY);
        assert_eq!(json["local"], false);
        assert_eq!(json["webviews"], serde_json::json!(["ytm"]));
        assert!(capability.windows.is_empty());
        assert!(capability.platforms.is_none());
        assert_eq!(json["permissions"], serde_json::json!(["allow-ytm-report"]));
        assert_eq!(
            json["remote"],
            serde_json::json!({ "urls": ["http://127.0.0.1:4100/*"] })
        );
        let pattern: RemoteUrlPattern = "http://127.0.0.1:4100/*".parse().unwrap();
        for url in [PAGE, "http://127.0.0.1:4100/index.html?x=1"] {
            assert!(pattern.test(&Url::parse(url).unwrap()), "{url}");
        }
        for url in [
            "http://127.0.0.1:4101/",
            "https://127.0.0.1:4100/",
            "http://localhost:4100/",
            "https://music.youtube.com/",
            "tauri://localhost/",
        ] {
            assert!(!pattern.test(&Url::parse(url).unwrap()), "{url}");
        }
    }

    #[test]
    fn release_has_no_dev_capability() {
        // Nothing checked in names a loopback or plain http origin.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        let mut files = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            files += 1;
            for needle in ["127.0.0.1", "localhost", "http://", "[::1]", DEV_CAPABILITY] {
                assert!(!text.contains(needle), "{}: {needle}", path.display());
            }
        }
        assert!(files >= 3);

        // The only runtime capability is added by `activate`, which exists for debug builds
        // only; the release `activate` does nothing.
        let source = include_str!("dev.rs");
        let code = &source[..source.find("#[cfg(test)]\nmod tests").unwrap()];
        assert_eq!(code.matches(".add_capability(").count(), 1);
        let debug = code
            .find("#[cfg(debug_assertions)]\npub fn activate")
            .unwrap();
        let release = code
            .find("#[cfg(not(debug_assertions))]\npub fn activate<R: tauri::Runtime, M: tauri::Manager<R>>(_app: &M) {}")
            .unwrap();
        let call = code.find(".add_capability(").unwrap();
        assert!(debug < call && call < release);
        assert!(code.contains("#[cfg(debug_assertions)]\npub fn capability("));

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
        let call = format!(".add_{}(", "capability");
        let callers: Vec<_> = files
            .iter()
            .filter(|file| {
                std::fs::read_to_string(file)
                    .unwrap()
                    .contains(call.as_str())
            })
            .collect();
        assert_eq!(callers.len(), 1, "{callers:?}");
        assert!(callers[0].ends_with("ytm/dev.rs"));
    }
}
