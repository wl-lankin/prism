//! Throwaway YouTube Music sign-in spike. Never merged.

use std::time::Duration;

use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder};

const YTM: &str = "https://music.youtube.com";
const SIGNIN: &str = "https://accounts.google.com/ServiceLogin?continue=https://music.youtube.com";

fn open(app: &AppHandle, label: &str, url: &str) -> Result<String, String> {
    if !cfg!(debug_assertions) {
        return Err("dev only".into());
    }
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.set_focus();
        return Ok(format!("{label} already open"));
    }
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("ytm-webview");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let parsed = url.parse::<Url>().map_err(|e| e.to_string())?;
    WebviewWindowBuilder::new(app, label, WebviewUrl::External(parsed))
        .title(format!("Prism spike: {label}"))
        .inner_size(1200.0, 800.0)
        .data_directory(dir.clone())
        .build()
        .map_err(|e| e.to_string())?;
    Ok(format!("opened {label} with data dir {}", dir.display()))
}

#[tauri::command]
pub async fn spike_open_ytm(app: AppHandle) -> Result<String, String> {
    open(&app, "ytm", YTM)
}

#[tauri::command]
pub async fn spike_open_google_signin(app: AppHandle) -> Result<String, String> {
    // Same label would clash; a distinct label shares the same data directory.
    open(&app, "ytm-signin", SIGNIN)
}

const PROBE_JS: &str = r#"
(function () {
  var md = navigator.mediaSession && navigator.mediaSession.metadata;
  var pp = document.querySelector('#play-pause-button');
  var nx = document.querySelector('.next-button');
  var out = {
    url: location.href,
    title: document.title,
    ua: navigator.userAgent,
    playbackState: navigator.mediaSession ? navigator.mediaSession.playbackState : null,
    metadata: md ? { title: md.title, artist: md.artist, album: md.album,
      artwork: Array.prototype.map.call(md.artwork || [], function (a) { return { src: a.src, sizes: a.sizes }; }) } : null,
    playPauseButton: !!pp, playPauseTitle: pp ? (pp.getAttribute('title') || pp.getAttribute('aria-label')) : null,
    nextButton: !!nx,
    videoPresent: !!document.querySelector('video'),
    videoPaused: document.querySelector('video') ? document.querySelector('video').paused : null
  };
  var json = JSON.stringify(out);
  history.replaceState(null, '', location.pathname + location.search + '#prismprobe=' + encodeURIComponent(json));
})();
"#;

#[tauri::command]
pub async fn spike_ytm_probe(app: AppHandle) -> Result<String, String> {
    if !cfg!(debug_assertions) {
        return Err("dev only".into());
    }
    let w = app.get_webview_window("ytm").ok_or("ytm window is not open")?;
    w.eval(PROBE_JS).map_err(|e| e.to_string())?;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(u) = w.url() {
            if let Some(f) = u.fragment().and_then(|f| f.strip_prefix("prismprobe=")) {
                let decoded: String = tauri::Url::parse(&format!("http://x/?v={f}"))
                    .ok()
                    .and_then(|x| x.query_pairs().next().map(|(_, v)| v.into_owned()))
                    .unwrap_or_else(|| f.to_string());
                return Ok(decoded);
            }
        }
    }
    Err("probe result not found in the URL fragment within 3s".into())
}
