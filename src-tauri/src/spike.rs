//! Throwaway YouTube Music sign-in spike. Never merged.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager, State, Url, WebviewUrl, WebviewWindowBuilder};

#[derive(Default)]
pub struct ProbeResult(pub Mutex<Option<String>>);

const YTM: &str = "https://music.youtube.com";
const SIGNIN: &str = "https://accounts.google.com/ServiceLogin?continue=https://music.youtube.com";

fn open(app: &AppHandle, label: &str, url: &str) -> Result<String, String> {
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

/// Called by the injected script when IPC works on the remote page.
#[tauri::command]
pub fn spike_probe_report(state: State<'_, ProbeResult>, json: String) {
    *state.0.lock().unwrap() = Some(json);
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
    videoPaused: document.querySelector('video') ? document.querySelector('video').paused : null,
    hasTauriIpc: !!(window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke)
  };
  var json = JSON.stringify(out);
  var sent = false;
  try {
    if (out.hasTauriIpc) {
      window.__TAURI_INTERNALS__.invoke('spike_probe_report', { json: json });
      sent = true;
    }
  } catch (e) {}
  if (!sent) { window.__prismProbe = json; }
  window.__prismProbeIpc = sent;
})();
"#;

#[tauri::command]
pub async fn spike_ytm_probe(app: AppHandle, state: State<'_, ProbeResult>) -> Result<String, String> {
    let w = app.get_webview_window("ytm").ok_or("ytm window is not open")?;
    *state.0.lock().unwrap() = None;
    w.eval(PROBE_JS).map_err(|e| e.to_string())?;
    for _ in 0..30 {
        tokio_sleep().await;
        if let Some(j) = state.0.lock().unwrap().take() {
            return Ok(j);
        }
    }
    Err("no report via IPC within 3s (remote IPC probably blocked)".into())
}

async fn tokio_sleep() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        let _ = tx.send(());
    });
    // Poll without blocking the async runtime for long.
    let _ = tauri::async_runtime::spawn_blocking(move || rx.recv()).await;
}
