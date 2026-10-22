//! The Google sign-in window (`ytm-signin`). Signing in inside the view fails, so Google's
//! own page gets a normal window that shares the view's data store; once it reaches YouTube
//! Music the session is there, the window closes and the view reloads.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, Url};

use super::policy::{self, Decision, From};
use super::view;

pub const SIGNIN_LABEL: &str = "ytm-signin";
/// Where Settings' "Sign in" starts.
pub const SIGNIN_URL: &str =
    "https://accounts.google.com/ServiceLogin?service=youtube&continue=https://music.youtube.com/";
pub const SIZE: (f64, f64) = (900.0, 700.0);

/// Held while the window is created, so two opens never build two windows. Never taken on
/// the main thread.
static OPENING: Mutex<()> = Mutex::new(());

/// The page the window starts on: an accounts.google.com page from the view, or the
/// standard YouTube sign-in.
pub fn start_url(from_view: Option<&Url>) -> Url {
    from_view
        .filter(|url| {
            url.host_str() == Some("accounts.google.com")
                && policy::decide(url, From::SignIn) == Decision::Stay
        })
        .cloned()
        .unwrap_or_else(|| Url::parse(SIGNIN_URL).expect("a valid sign-in URL"))
}

/// What the window does with a navigation. Runs inside the web view's callback.
fn act(app: &AppHandle, url: &Url) -> bool {
    match policy::decide(url, From::SignIn) {
        Decision::Stay => true,
        Decision::External | Decision::SignIn => {
            let allowed = app.try_state::<crate::AppState>().is_some_and(|state| {
                state
                    .ytm_view
                    .may_hand_off(From::SignIn, std::time::Instant::now())
            });
            if allowed {
                view::open_outside(app, url);
            }
            false
        }
        Decision::Block => false,
    }
}

/// Opens the window, or brings the open one to the front. Creates a window, so not for the
/// main thread (WebView2 deadlocks there). Only through `YtmView::open_sign_in`, which
/// creates it under the view's creation lock (the two share a store).
pub fn open(app: &AppHandle, data_dir: &Path, url: Option<Url>) -> Result<(), String> {
    let _opening = OPENING.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(window) = app.get_webview_window(SIGNIN_LABEL) {
        let fail = |err: tauri::Error| err.to_string();
        window.show().map_err(fail)?;
        if window.is_minimized().unwrap_or(false) {
            window.unminimize().map_err(fail)?;
        }
        return window.set_focus().map_err(fail);
    }
    let done: view::Done = Arc::new(AtomicBool::new(false));
    let on_navigation = {
        let app = app.clone();
        move |url: &Url| act(&app, url)
    };
    let on_new_window = {
        let app = app.clone();
        move |url: Url, _features| {
            if policy::decide(&url, From::SignIn) != Decision::Block {
                // Help and privacy links in their own window: the system browser.
                let allowed = app.try_state::<crate::AppState>().is_some_and(|state| {
                    state
                        .ytm_view
                        .may_hand_off(From::SignIn, std::time::Instant::now())
                });
                if allowed {
                    view::open_outside(&app, &url);
                }
            }
            tauri::webview::NewWindowResponse::Deny
        }
    };
    let on_page_load = {
        let app = app.clone();
        move |_window: tauri::WebviewWindow, payload: tauri::webview::PageLoadPayload<'_>| {
            // Started: the response is in, so every cookie of the way here is stored.
            if payload.event() == tauri::webview::PageLoadEvent::Started
                && policy::sign_in_done(payload.url())
            {
                view::finish_sign_in(&app, &done);
            }
        }
    };
    let builder = tauri::WebviewWindowBuilder::new(
        app,
        SIGNIN_LABEL,
        tauri::WebviewUrl::External(start_url(url.as_ref())),
    )
    .title("Google")
    .inner_size(SIZE.0, SIZE.1)
    .center()
    .resizable(true)
    .decorations(true)
    .data_store_identifier(view::YTM_STORE)
    .on_navigation(on_navigation)
    .on_new_window(on_new_window)
    .on_page_load(on_page_load)
    .on_document_title_changed(|window, title| {
        let _ = window.set_title(&title);
    });
    let builder = match super::agent::user_agent() {
        Some(agent) => builder.user_agent(&agent),
        None => builder,
    };
    // Windows: the view's own profile, with the same switches (WebView2 requires them to
    // match per profile).
    let builder = if cfg!(windows) {
        builder
            .data_directory(data_dir.join(view::DATA_DIR_NAME))
            .additional_browser_args(view::BROWSER_ARGS)
    } else {
        builder
    };
    builder.build().map(|_| ()).map_err(|err| err.to_string())
}

pub fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(SIGNIN_LABEL) {
        if let Err(err) = window.close() {
            eprintln!("closing the Google sign-in failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_starts_on_google_accounts_only() {
        let standard = Url::parse(SIGNIN_URL).unwrap();
        assert_eq!(standard.host_str(), Some("accounts.google.com"));
        assert_eq!(
            policy::decide(&standard, From::View),
            Decision::SignIn,
            "the standard page is a sign-in page"
        );
        assert_eq!(policy::decide(&standard, From::SignIn), Decision::Stay);
        assert_eq!(start_url(None), standard);

        let from_view = Url::parse(
            "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fwww.youtube.com%2Fsignin",
        )
        .unwrap();
        assert_eq!(start_url(Some(&from_view)), from_view);
        for other in [
            "https://example.org/",
            "https://myaccount.google.com/",
            "http://accounts.google.com/ServiceLogin",
            "https://accounts.google.com:8443/ServiceLogin",
            "https://tauri.localhost/",
        ] {
            let url = Url::parse(other).unwrap();
            assert_eq!(start_url(Some(&url)), standard, "{other}");
        }
        assert_eq!(SIGNIN_LABEL, "ytm-signin");
        assert_eq!(SIZE, (900.0, 700.0));
    }
}
