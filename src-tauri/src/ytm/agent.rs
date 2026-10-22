//! The user agent the YouTube Music view and the sign-in window send.
//!
//! WKWebView is Safari's engine but leaves the `Version/... Safari/...` tokens out of its user
//! agent, and YouTube Music answers such a browser with "not optimized for your browser"
//! (owner check, 2026-10-22). On macOS both windows therefore send the user agent of the
//! Safari installed on this Mac. WebView2's own user agent is accepted, so Windows keeps it.

/// Safari's user agent keeps the OS part frozen at 10_15_7 and the WebKit part at 605.1.15.
const SAFARI_UA_HEAD: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const SAFARI_INFO: &str = "/Applications/Safari.app/Contents/Info.plist";
/// Used when Safari's version cannot be read.
const FALLBACK_VERSION: &str = "18.0";

/// The user agent for the YouTube Music windows, `None` to keep the platform's own.
pub fn user_agent() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let version = std::fs::read_to_string(SAFARI_INFO)
        .ok()
        .and_then(|plist| short_version(&plist))
        .unwrap_or_else(|| FALLBACK_VERSION.to_string());
    Some(safari_user_agent(&version))
}

fn safari_user_agent(version: &str) -> String {
    format!("{SAFARI_UA_HEAD} Version/{version} Safari/605.1.15")
}

/// `CFBundleShortVersionString` from an XML Info.plist; only digits and dots are accepted.
fn short_version(plist: &str) -> Option<String> {
    let after_key = plist
        .split("<key>CFBundleShortVersionString</key>")
        .nth(1)?;
    let value = after_key
        .split("<string>")
        .nth(1)?
        .split("</string>")
        .next()?;
    let value = value.trim();
    let valid = !value.is_empty()
        && value.len() <= 16
        && value.chars().all(|c| c.is_ascii_digit() || c == '.');
    valid.then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_safari_version_from_the_plist() {
        let plist =
            "<dict>\n\t<key>CFBundleShortVersionString</key>\n\t<string>27.0.1</string>\n</dict>";
        assert_eq!(short_version(plist).as_deref(), Some("27.0.1"));
    }

    #[test]
    fn odd_versions_are_refused() {
        let bad = "<key>CFBundleShortVersionString</key><string>27.0 beta\"</string>";
        assert_eq!(short_version(bad), None);
        assert_eq!(short_version("<dict></dict>"), None);
    }

    #[test]
    fn user_agent_looks_like_safari() {
        assert_eq!(
            safari_user_agent("27.0.1"),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0.1 Safari/605.1.15"
        );
    }

    #[test]
    fn only_macos_changes_the_user_agent() {
        assert_eq!(user_agent().is_some(), cfg!(target_os = "macos"));
    }
}
