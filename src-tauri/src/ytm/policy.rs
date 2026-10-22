//! Where the YouTube Music view and the sign-in window may go.
//!
//! Only https reaches anything. Prism's own origins (custom schemes, `*.localhost`) and IP
//! addresses are blocked and never handed to the system browser. The view stays on YouTube
//! Music's own hosts; Google's sign-in goes to the sign-in window; any other page opens in
//! the system browser.

use tauri::Url;

use super::PAGE_HOST;

/// Which web view asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum From {
    /// The `ytm` child view in the main window.
    View,
    /// The `ytm-signin` window.
    SignIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The web view loads it.
    Stay,
    /// The sign-in window opens with it; the view does not navigate.
    SignIn,
    /// The system browser opens it; the web view does not navigate.
    External,
    /// Nothing happens.
    Block,
}

const ACCOUNTS_HOST: &str = "accounts.google.com";

/// Pages of accounts.google.com that YouTube Music loads by itself, with no sign-in form.
const ACCOUNTS_BACKGROUND: [&str; 5] = [
    "/RotateCookiesPage",
    "/CheckCookie",
    "/ListAccounts",
    "/Logout",
    "/o/oauth2/",
];

/// Paths of www.youtube.com that YouTube Music redirects through on sign-in and sign-out.
const YOUTUBE_REDIRECTS: [&str; 2] = ["/signin", "/logout"];

/// Ad and analytics frames: dropped quietly, so a frame never opens the system browser
/// (WKWebView asks about every frame's navigation, not only the page's).
const QUIET_DOMAINS: [&str; 6] = [
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "google-analytics.com",
    "googletagmanager.com",
    "gstatic.com",
];

/// Domains the sign-in window keeps (Google's sign-in, its help pages and resources, and
/// the YouTube redirects that carry the session over).
const SIGN_IN_DOMAINS: [&str; 4] = ["google.com", "youtube.com", "gstatic.com", "googleapis.com"];

fn under(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|rest| rest.ends_with('.'))
}

fn starts_with_segment(path: &str, prefix: &str) -> bool {
    match path.strip_prefix(prefix) {
        Some(rest) => prefix.ends_with('/') || rest.is_empty() || rest.starts_with(['/', '?']),
        None => false,
    }
}

/// Names that only mean something inside a home or office network (a router, a NAS).
const LOCAL_DOMAINS: [&str; 5] = ["local", "lan", "home.arpa", "box", "internal"];

/// Prism's own origins (`tauri.localhost`, `ipc.localhost`, `prism.localhost`,
/// `localhost`), single-label names and local network names: never a page to open.
fn is_app_or_local_host(host: &str) -> bool {
    under(host, "localhost")
        || !host.contains('.')
        || LOCAL_DOMAINS.iter().any(|domain| under(host, domain))
}

pub fn decide(url: &Url, from: From) -> Decision {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Decision::Block;
    }
    let Some(host) = url.host_str() else {
        return Decision::Block;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let ip = host.starts_with('[') || host.parse::<std::net::Ipv4Addr>().is_ok();
    if host.is_empty() || ip || is_app_or_local_host(&host) {
        return Decision::Block;
    }
    // An explicit port is never one of Google's pages.
    let default_port = url.port().is_none();
    match from {
        From::View => {
            let path = url.path();
            match host.as_str() {
                _ if !default_port => Decision::External,
                PAGE_HOST
                | "consent.youtube.com"
                | "consent.google.com"
                | "accounts.youtube.com" => Decision::Stay,
                "www.youtube.com"
                    if YOUTUBE_REDIRECTS
                        .iter()
                        .any(|prefix| starts_with_segment(path, prefix)) =>
                {
                    Decision::Stay
                }
                ACCOUNTS_HOST
                    if ACCOUNTS_BACKGROUND
                        .iter()
                        .any(|prefix| starts_with_segment(path, prefix)) =>
                {
                    Decision::Stay
                }
                ACCOUNTS_HOST => Decision::SignIn,
                _ if QUIET_DOMAINS.iter().any(|domain| under(&host, domain)) => Decision::Block,
                _ => Decision::External,
            }
        }
        From::SignIn => {
            if default_port && SIGN_IN_DOMAINS.iter().any(|domain| under(&host, domain)) {
                Decision::Stay
            } else if QUIET_DOMAINS.iter().any(|domain| under(&host, domain)) {
                Decision::Block
            } else {
                Decision::External
            }
        }
    }
}

/// The sign-in window reached YouTube Music: the session is in the shared store now.
pub fn sign_in_done(url: &Url) -> bool {
    url.scheme() == "https" && url.host_str() == Some(PAGE_HOST) && url.port().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    fn both(raw: &str) -> (Decision, Decision) {
        let url = url(raw);
        (decide(&url, From::View), decide(&url, From::SignIn))
    }

    #[test]
    fn policy_keeps_youtube_music_hosts() {
        for raw in [
            "https://music.youtube.com/",
            "https://music.youtube.com/watch?v=abc&list=RD1",
            "https://MUSIC.youtube.com/library",
            "https://consent.youtube.com/m?continue=https://music.youtube.com/",
            "https://consent.google.com/ml?continue=https://music.youtube.com/&gl=DE",
            "https://accounts.youtube.com/accounts/SetSID?ssdc=1",
            "https://accounts.youtube.com/RotateCookiesPage?origin=https://music.youtube.com",
            "https://www.youtube.com/signin?action_handle_signin=true&next=%2F",
            "https://www.youtube.com/logout",
            "https://accounts.google.com/RotateCookiesPage?og_pid=1",
            "https://accounts.google.com/CheckCookie?continue=x",
            "https://accounts.google.com/Logout?continue=https://music.youtube.com/",
            "https://accounts.google.com/o/oauth2/postmessageRelay",
        ] {
            assert_eq!(decide(&url(raw), From::View), Decision::Stay, "{raw}");
        }
        // Other YouTube pages are not the music player.
        for raw in [
            "https://www.youtube.com/",
            "https://www.youtube.com/watch?v=abc",
            "https://www.youtube.com/signinx",
            "https://www.youtube.com/premium",
            "https://www.youtube.com/redirect?q=https://example.org/",
            "https://youtube.com/",
            "https://m.youtube.com/",
            "https://music.youtube.com:8443/",
            "https://music.youtube.com.example.org/",
            "https://evilmusic.youtube.com.example.org/",
        ] {
            assert_eq!(decide(&url(raw), From::View), Decision::External, "{raw}");
        }
        // The sign-in window keeps Google's and YouTube's pages, and finishes on YouTube
        // Music.
        for raw in [
            "https://accounts.google.com/v3/signin/identifier?continue=x",
            "https://accounts.google.com/signin/challenge/pwd",
            "https://myaccount.google.com/",
            "https://support.google.com/accounts/answer/1",
            "https://www.gstatic.com/recaptcha/x.js",
            "https://accounts.youtube.com/accounts/SetSID",
            "https://www.youtube.com/signin?next=%2F",
            "https://music.youtube.com/",
        ] {
            assert_eq!(decide(&url(raw), From::SignIn), Decision::Stay, "{raw}");
        }
        assert!(sign_in_done(&url("https://music.youtube.com/?a=1")));
        for raw in [
            "https://www.youtube.com/signin",
            "https://accounts.google.com/",
            "http://music.youtube.com/",
            "https://music.youtube.com:444/",
            "https://music.youtube.com.example.org/",
        ] {
            assert!(!sign_in_done(&url(raw)), "{raw}");
        }
    }

    #[test]
    fn policy_sends_google_accounts_to_the_sign_in_window() {
        for raw in [
            "https://accounts.google.com/ServiceLogin?service=youtube&uilel=3&passive=true\
             &continue=https%3A%2F%2Fwww.youtube.com%2Fsignin",
            "https://accounts.google.com/",
            "https://accounts.google.com/v3/signin/identifier",
            "https://accounts.google.com/AccountChooser?continue=x",
            "https://accounts.google.com/AddSession",
            "https://ACCOUNTS.google.com/ServiceLogin",
            // Not one of the background pages, despite the prefix.
            "https://accounts.google.com/LogoutX",
        ] {
            assert_eq!(decide(&url(raw), From::View), Decision::SignIn, "{raw}");
            // Inside the sign-in window it simply loads.
            assert_eq!(decide(&url(raw), From::SignIn), Decision::Stay, "{raw}");
        }
        // Lookalikes are not Google's accounts page.
        for raw in [
            "https://accounts.google.com.example.org/ServiceLogin",
            "https://accounts-google.com/ServiceLogin",
            "https://accounts.google.com:8443/ServiceLogin",
        ] {
            assert_eq!(both(raw), (Decision::External, Decision::External), "{raw}");
        }
    }

    #[test]
    fn policy_opens_other_links_outside() {
        for raw in [
            "https://example.org/",
            "https://shop.example.com/merch?artist=Die%20Testband",
            "https://www.wikipedia.org/wiki/Testhausen",
            "https://xn--mnchen-3ya.example/",
        ] {
            assert_eq!(both(raw), (Decision::External, Decision::External), "{raw}");
        }
        // Help pages are external from the view, but stay in the sign-in window.
        assert_eq!(
            both("https://support.google.com/youtubemusic"),
            (Decision::External, Decision::Stay)
        );
        // Ad and analytics frames never reach the system browser from the view.
        for raw in [
            "https://googleads.g.doubleclick.net/pagead/id",
            "https://tpc.googlesyndication.com/sodar/sodar2.html",
            "https://www.googleadservices.com/pagead/conversion",
            "https://www.gstatic.com/x.html",
        ] {
            assert_eq!(decide(&url(raw), From::View), Decision::Block, "{raw}");
        }
        assert_eq!(
            both("https://googleads.g.doubleclick.net/pagead/id"),
            (Decision::Block, Decision::Block)
        );
    }

    #[test]
    fn policy_blocks_non_web_schemes() {
        for raw in [
            "http://music.youtube.com/",
            "http://example.org/",
            "file:///etc/passwd",
            "data:text/html,<script>alert(1)</script>",
            "blob:https://music.youtube.com/0f1e",
            "javascript:alert(1)",
            "about:blank",
            "about:srcdoc",
            "ftp://example.org/",
            "ws://example.org/",
            "wss://example.org/",
            "mailto:demo@example.org",
            "intent://x#Intent;end",
            "itms-apps://apps.apple.com/",
            "chrome://settings",
            "edge://settings",
            "view-source:https://music.youtube.com/",
            "prism://localhost/cover/1",
            "asset://localhost/x",
        ] {
            assert_eq!(both(raw), (Decision::Block, Decision::Block), "{raw}");
        }
        // Userinfo and IP addresses.
        for raw in [
            "https://music.youtube.com@example.org/",
            "https://demo:demo-pass@music.youtube.com/",
            "https://127.0.0.1/",
            "https://192.168.0.1/",
            "https://[::1]/",
            "https://8.8.8.8/",
            "https://0x7f.1/",
        ] {
            assert_eq!(both(raw), (Decision::Block, Decision::Block), "{raw}");
        }
    }

    #[test]
    fn policy_blocks_app_origins() {
        for raw in [
            "tauri://localhost/",
            "tauri://localhost/index.html",
            "http://localhost:1440/",
            "http://tauri.localhost/",
            "https://tauri.localhost/",
            "https://tauri.localhost./",
            "https://TAURI.localhost/",
            "ipc://localhost/",
            "http://ipc.localhost/ytm_report",
            "https://ipc.localhost/ytm_report",
            "https://prism.localhost/cover/1",
            "https://localhost/",
            "https://localhost:1440/",
            "https://x.y.localhost/",
        ] {
            assert_eq!(both(raw), (Decision::Block, Decision::Block), "{raw}");
        }
    }

    #[test]
    fn policy_blocks_local_network_names() {
        for raw in [
            "https://router/",
            "https://nas/admin",
            "https://printer./",
            "https://nas.local/",
            "https://Testhausen-NAS.LOCAL/",
            "https://pi.lan/",
            "https://media.home.arpa/",
            "https://fritz.box/",
            "https://www.fritz.box/",
            "https://wiki.internal/",
        ] {
            assert_eq!(both(raw), (Decision::Block, Decision::Block), "{raw}");
        }
        // Lookalikes on public names still open outside.
        for raw in [
            "https://local.example.org/",
            "https://lan.example/",
            "https://fritzbox.example/",
            "https://box.example.com/",
        ] {
            assert_eq!(both(raw), (Decision::External, Decision::External), "{raw}");
        }
    }
}
