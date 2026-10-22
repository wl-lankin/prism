//! YouTube Music inside Prism: what its page reports, the state Prism shows for it, and the
//! commands Prism sends into the page.
//!
//! The page is untrusted. It reaches Prism only through `ytm_report` (see [`report_allowed`]
//! and [`report::parse`]), and Prism talks back only with the fixed [`SNIPPETS`]; a seek or a
//! volume sets one number, formatted by Rust, before its snippet runs.

pub mod covers;
pub mod dev;
pub mod policy;
pub mod report;
pub mod signin;
pub mod state;
#[cfg(test)]
#[path = "testpage/server.rs"]
mod testpage;
pub mod view;

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use covers::Artwork;
use report::{PageState, Report};
use state::{YtmAction, YtmControl, YtmState, YtmStatus};

/// The label of the child web view that shows music.youtube.com.
pub const VIEW_LABEL: &str = "ytm";
pub const PAGE_HOST: &str = "music.youtube.com";
/// Payload: [`YtmState`]. Sent to the main and the mini page, never to the YouTube Music view.
pub const EVENT_STATE: &str = "ytm://state";

const BRIDGE: &str = include_str!("bridge.js");

/// The bridge as the view's initialization script. The block keeps `ALLOW_DEV_HOST` out of
/// the page's global scope.
pub fn bridge_script(allow_dev_host: bool) -> String {
    format!("{{ const ALLOW_DEV_HOST = {allow_dev_host};\n{BRIDGE}\n}}")
}

#[cfg(debug_assertions)]
static DEV_HOST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Debug builds only: lets a page on `http://127.0.0.1` report, for the fake YouTube Music
/// page of dev runs.
#[cfg(debug_assertions)]
pub fn allow_dev_host(on: bool) {
    DEV_HOST.store(on, std::sync::atomic::Ordering::SeqCst);
}

/// Whether a page on `http://127.0.0.1` may report; always false in release builds.
pub fn dev_host() -> bool {
    #[cfg(debug_assertions)]
    {
        DEV_HOST.load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

/// A report counts only from the `ytm` view while it shows music.youtube.com over https
/// (or, with `dev`, a page on `http://127.0.0.1`).
pub fn report_allowed(label: &str, url: Option<&tauri::Url>, dev: bool) -> bool {
    let Some(url) = url else {
        return false;
    };
    let host = url.host_str();
    let page = url.scheme() == "https" && host == Some(PAGE_HOST) && url.port().is_none();
    let fake = dev && url.scheme() == "http" && host == Some("127.0.0.1");
    label == VIEW_LABEL && (page || fake)
}

/// The only script Prism sends into the page, by action name.
pub const SNIPPETS: [(&str, &str); 7] = [
    (
        "toggle",
        "window.dispatchEvent(new CustomEvent('prism-ytm', { detail: 'toggle' }));",
    ),
    (
        "play",
        "window.dispatchEvent(new CustomEvent('prism-ytm', { detail: 'play' }));",
    ),
    (
        "pause",
        "window.dispatchEvent(new CustomEvent('prism-ytm', { detail: 'pause' }));",
    ),
    (
        "next",
        "window.dispatchEvent(new CustomEvent('prism-ytm', { detail: 'next' }));",
    ),
    (
        "prev",
        "window.dispatchEvent(new CustomEvent('prism-ytm', { detail: 'prev' }));",
    ),
    (
        "seek",
        "window.dispatchEvent(new CustomEvent('prism-ytm', { detail: 'seek' }));",
    ),
    (
        "volume",
        "window.dispatchEvent(new CustomEvent('prism-ytm', { detail: 'volume' }));",
    ),
];

fn snippet(name: &str) -> &'static str {
    SNIPPETS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, script)| *script)
        .unwrap_or("")
}

/// The scripts one action sends, in order. Seek and volume first set their number (an
/// integer, or a decimal with three places); a volume that is not a number sends nothing.
pub fn scripts(action: YtmAction) -> Vec<String> {
    match action {
        YtmAction::Toggle => vec![snippet("toggle").into()],
        YtmAction::Play => vec![snippet("play").into()],
        YtmAction::Pause => vec![snippet("pause").into()],
        YtmAction::Next => vec![snippet("next").into()],
        YtmAction::Prev => vec![snippet("prev").into()],
        YtmAction::Seek(ms) => vec![
            format!("window.__prismSeekMs = {};", ms.min(report::MAX_MS)),
            snippet("seek").into(),
        ],
        YtmAction::Volume(level) if level.is_finite() => {
            // Also turns -0.0 into 0.
            let level = if level > 0.0 { level.min(1.0) } else { 0.0 };
            vec![
                format!("window.__prismVolume = {level:.3};"),
                snippet("volume").into(),
            ]
        }
        YtmAction::Volume(_) => Vec::new(),
    }
}

/// `ytm_control`'s arguments as an action. `seek` needs `position_ms`, `volume` needs
/// `volume` (0..=1, clamped); the other actions ignore both.
pub fn parse_action(
    action: &str,
    position_ms: Option<f64>,
    volume: Option<f64>,
) -> Result<YtmAction, String> {
    Ok(match action {
        "toggle" => YtmAction::Toggle,
        "play" => YtmAction::Play,
        "pause" => YtmAction::Pause,
        "next" => YtmAction::Next,
        "prev" => YtmAction::Prev,
        "seek" => match position_ms {
            Some(ms) if ms.is_finite() && ms >= 0.0 => {
                YtmAction::Seek((ms.round() as u64).min(report::MAX_MS))
            }
            _ => return Err("A seek needs a position in milliseconds.".to_string()),
        },
        "volume" => match volume {
            Some(level) if level.is_finite() => YtmAction::Volume(level.clamp(0.0, 1.0) as f32),
            _ => return Err("The volume must be a number between 0 and 1.".to_string()),
        },
        _ => return Err(format!("Unknown YouTube Music action: {action}")),
    })
}

/// Runs a script in the YouTube Music view, if it exists. Must not block.
pub trait PageEval: Send + Sync {
    fn eval(&self, script: &str);
}

/// The `ytm` child web view, looked up by label on every call; no view, no eval.
pub struct WebviewEval(pub tauri::AppHandle);

impl PageEval for WebviewEval {
    fn eval(&self, script: &str) {
        use tauri::Manager;
        if let Some(view) = self.0.get_webview(VIEW_LABEL) {
            // Posts to the event loop; it does not wait for the page.
            if let Err(err) = view.eval(script) {
                eprintln!("sending a command to YouTube Music failed: {err}");
            }
        }
    }
}

/// Called with the new state whenever it changes (for `ytm://state`).
pub type StateEmit = Arc<dyn Fn(&YtmState) + Send + Sync>;
/// Called with every state a report gives, unchanged ones included (for the transport).
pub type StateFollower = Arc<dyn Fn(&YtmState) + Send + Sync>;

/// Reports the page may send per second on average; the excess is dropped.
pub const REPORTS_PER_SEC: f64 = 20.0;
/// Reports the page may send at once.
pub const REPORT_BURST: f64 = 10.0;

/// A token bucket for the page's reports.
struct Bucket {
    tokens: f64,
    at: Option<Instant>,
}

impl Bucket {
    fn full() -> Bucket {
        Bucket {
            tokens: REPORT_BURST,
            at: None,
        }
    }

    fn take(&mut self, now: Instant) -> bool {
        if let Some(at) = self.at {
            let elapsed = now.saturating_duration_since(at).as_secs_f64();
            self.tokens = (self.tokens + elapsed * REPORTS_PER_SEC).min(REPORT_BURST);
        }
        if self.at.is_none_or(|at| now > at) {
            self.at = Some(now);
        }
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

struct Inner {
    state: YtmState,
    /// `playing` of the last report let through.
    playing: bool,
    bucket: Bucket,
}

pub struct YtmService {
    page: Arc<dyn PageEval>,
    emit: StateEmit,
    artwork: Arc<Artwork>,
    inner: Mutex<Inner>,
    /// Held from a state's change to its emit and follow, so they never go out of order.
    order: Mutex<()>,
    follower: Mutex<Option<StateFollower>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl YtmService {
    pub fn new(page: Arc<dyn PageEval>, emit: StateEmit) -> YtmService {
        YtmService {
            page,
            emit,
            artwork: Arc::default(),
            inner: Mutex::new(Inner {
                state: YtmState::default(),
                playing: false,
                bucket: Bucket::full(),
            }),
            order: Mutex::new(()),
            follower: Mutex::new(None),
        }
    }

    /// The artwork URLs behind the `yt-` cover ids.
    pub fn artwork(&self) -> Arc<Artwork> {
        Arc::clone(&self.artwork)
    }

    /// Replaces the follower. It is called in order, outside the state's lock, and must not
    /// call back into the service's `handle` or `reset`.
    pub fn follow(&self, follower: StateFollower) {
        *lock(&self.follower) = Some(follower);
    }

    pub fn state(&self) -> YtmState {
        lock(&self.inner).state.clone()
    }

    /// Applies one validated report, unless the page sends more than [`REPORTS_PER_SEC`]
    /// (bursts of [`REPORT_BURST`]); the first report after a change of `playing` always
    /// counts, so a start or a pause is never lost.
    pub fn handle(&self, report: Report) {
        self.handle_at(report, Instant::now());
    }

    fn handle_at(&self, report: Report, now: Instant) {
        let _order = lock(&self.order);
        let page = match report {
            Report::State(page) => Some(page),
            Report::Ended => None,
        };
        {
            let mut inner = lock(&self.inner);
            let edge = page
                .as_ref()
                .is_some_and(|page| page.playing != inner.playing);
            if !inner.bucket.take(now) && !edge {
                return;
            }
            if let Some(page) = &page {
                inner.playing = page.playing;
            }
        }
        // `ended` changes nothing by itself: the state report that follows carries it.
        if let Some(page) = page {
            let next = self.derive(&page);
            self.apply(next);
        }
    }

    /// Back to idle: the view is gone (sign-out, destroy), so control falls back to Prism.
    pub fn reset(&self) {
        let _order = lock(&self.order);
        lock(&self.inner).playing = false;
        self.apply(YtmState::default());
    }

    /// Called with `order` held.
    fn apply(&self, next: YtmState) {
        let changed = {
            let mut inner = lock(&self.inner);
            let changed = inner.state != next;
            inner.state = next.clone();
            changed
        };
        if changed {
            (self.emit)(&next);
        }
        let follower = lock(&self.follower).clone();
        if let Some(follower) = follower {
            follower(&next);
        }
    }

    fn derive(&self, page: &PageState) -> YtmState {
        let status = if !page.available {
            YtmStatus::Unavailable
        } else if page.playing {
            YtmStatus::Playing
        } else if page.title.is_empty() && page.duration_ms == 0 {
            YtmStatus::Idle
        } else {
            YtmStatus::Paused
        };
        YtmState {
            status,
            title: page.title.clone(),
            artist: page.artist.clone(),
            album: page.album.clone(),
            cover_id: page
                .artwork
                .as_deref()
                .map(|url| self.artwork.remember(url)),
            position_ms: page.position_ms,
            duration_ms: page.duration_ms,
            can_next: page.can_next,
            can_prev: page.can_prev,
        }
    }
}

impl YtmControl for YtmService {
    fn send(&self, action: YtmAction) {
        for script in scripts(action) {
            self.page.eval(&script);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::report::tests::{bridge_report, ART};
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    impl PageEval for Recorder {
        fn eval(&self, script: &str) {
            lock(&self.0).push(script.to_string());
        }
    }

    struct Fixture {
        service: YtmService,
        page: Arc<Recorder>,
        emitted: Arc<Mutex<Vec<YtmState>>>,
        followed: Arc<Mutex<Vec<YtmState>>>,
    }

    fn fixture() -> Fixture {
        let page = Arc::new(Recorder::default());
        let emitted: Arc<Mutex<Vec<YtmState>>> = Arc::default();
        let followed: Arc<Mutex<Vec<YtmState>>> = Arc::default();
        let service = YtmService::new(Arc::clone(&page) as Arc<dyn PageEval>, {
            let emitted = Arc::clone(&emitted);
            Arc::new(move |state| lock(&emitted).push(state.clone()))
        });
        service.follow({
            let followed = Arc::clone(&followed);
            Arc::new(move |state| lock(&followed).push(state.clone()))
        });
        Fixture {
            service,
            page,
            emitted,
            followed,
        }
    }

    fn report_with(changes: serde_json::Value) -> Report {
        let mut value = bridge_report();
        for (key, field) in changes.as_object().unwrap() {
            value[key] = field.clone();
        }
        report::parse(value).unwrap()
    }

    #[test]
    fn state_follows_reports() {
        let f = fixture();
        assert_eq!(f.service.state(), YtmState::default());

        f.service.handle(report_with(json!({})));
        let state = f.service.state();
        assert_eq!(state.status, YtmStatus::Playing);
        assert_eq!(state.title, "Testlied");
        assert_eq!(state.artist, "Die Testband");
        assert_eq!(state.album, "Testhausen");
        assert_eq!(state.cover_id, Some(covers::cover_id(ART)));
        assert_eq!((state.position_ms, state.duration_ms), (12_000, 180_000));
        assert!(state.can_next && !state.can_prev);
        assert_eq!(lock(&f.emitted).len(), 1);
        assert_eq!(lock(&f.followed).len(), 1);
        let raw = covers::ytm_cover_raw(state.cover_id.as_deref().unwrap()).unwrap();
        assert_eq!(f.service.artwork().url_of(raw).as_deref(), Some(ART));

        // A heartbeat with nothing new: no event, but the transport hears it.
        f.service.handle(report_with(json!({})));
        assert_eq!(lock(&f.emitted).len(), 1);
        assert_eq!(lock(&f.followed).len(), 2);

        // The position moved: an event.
        f.service
            .handle(report_with(json!({ "positionMs": 13_000 })));
        assert_eq!(lock(&f.emitted).len(), 2);
        assert_eq!(f.service.state().position_ms, 13_000);

        // `ended` changes nothing by itself.
        f.service.handle(Report::Ended);
        assert_eq!(lock(&f.emitted).len(), 2);
        assert_eq!(lock(&f.followed).len(), 3);

        f.service.handle(report_with(json!({ "playing": false })));
        assert_eq!(f.service.state().status, YtmStatus::Paused);

        f.service.handle(report_with(json!({
            "playing": false, "title": "", "artist": "", "album": "", "artwork": "",
            "positionMs": 0, "durationMs": 0,
        })));
        let state = f.service.state();
        assert_eq!(state.status, YtmStatus::Idle);
        assert_eq!(state.cover_id, None);
        assert_eq!(lock(&f.emitted).last(), Some(&state));
        assert_eq!(lock(&f.followed).last(), Some(&state));

        f.service.handle(report_with(json!({})));
        f.service.reset();
        assert_eq!(f.service.state(), YtmState::default());
        assert_eq!(lock(&f.emitted).last(), Some(&YtmState::default()));
        assert_eq!(lock(&f.followed).last(), Some(&YtmState::default()));

        // Handling reports never sends anything into the page.
        assert!(lock(&f.page.0).is_empty());
    }

    #[test]
    fn reports_are_rate_limited_but_play_changes_always_count() {
        use std::time::Duration;
        let f = fixture();
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let followed = || lock(&f.followed).len();

        // A burst of 10 at once counts; the 11th is dropped silently.
        for n in 0..10 {
            f.service
                .handle_at(report_with(json!({ "positionMs": n * 10 })), at(0));
        }
        assert_eq!(followed(), 10);
        f.service
            .handle_at(report_with(json!({ "positionMs": 500 })), at(0));
        assert_eq!(followed(), 10);
        assert_eq!(f.service.state().position_ms, 90);
        f.service.handle_at(Report::Ended, at(0));

        // Out of tokens, a pause still counts, once; the next one with the same play state
        // does not.
        f.service
            .handle_at(report_with(json!({ "playing": false })), at(0));
        assert_eq!(followed(), 11);
        assert_eq!(f.service.state().status, YtmStatus::Paused);
        f.service.handle_at(
            report_with(json!({ "playing": false, "title": "x" })),
            at(0),
        );
        assert_eq!(followed(), 11);
        assert_eq!(f.service.state().title, "Testlied");
        // So does the start after it.
        f.service.handle_at(report_with(json!({})), at(0));
        assert_eq!(followed(), 12);
        assert_eq!(f.service.state().status, YtmStatus::Playing);

        // 20 per second: one more after 60 ms, and never more than the burst after a pause.
        f.service.handle_at(report_with(json!({})), at(10));
        assert_eq!(followed(), 12);
        f.service.handle_at(report_with(json!({})), at(60));
        assert_eq!(followed(), 13);
        for _ in 0..15 {
            f.service.handle_at(report_with(json!({})), at(60_000));
        }
        assert_eq!(followed(), 23);

        // A flapping page gets every edge through, but nothing in between.
        for n in 0..6 {
            let playing = n % 2 == 1;
            f.service
                .handle_at(report_with(json!({ "playing": playing })), at(60_000));
            f.service
                .handle_at(report_with(json!({ "playing": playing })), at(60_000));
        }
        assert_eq!(followed(), 29);

        // Reset is never limited, and the next start after it counts.
        f.service.reset();
        assert_eq!(followed(), 30);
        f.service.handle_at(report_with(json!({})), at(60_000));
        assert_eq!(followed(), 31);
    }

    #[test]
    fn state_emits_never_go_out_of_order() {
        use std::sync::OnceLock;
        use std::time::Duration;
        let service_slot: Arc<OnceLock<Arc<YtmService>>> = Arc::default();
        let mismatches: Arc<Mutex<usize>> = Arc::default();
        let emitted: Arc<Mutex<Vec<YtmState>>> = Arc::default();
        let service = Arc::new(YtmService::new(Arc::new(Recorder::default()), {
            let (slot, mismatches, emitted) = (
                Arc::clone(&service_slot),
                Arc::clone(&mismatches),
                Arc::clone(&emitted),
            );
            Arc::new(move |state| {
                // Give a racing apply time to overtake this emit.
                std::thread::yield_now();
                if let Some(service) = slot.get() {
                    if service.state() != *state {
                        *lock(&mismatches) += 1;
                    }
                }
                lock(&emitted).push(state.clone());
            })
        }));
        let _ = service_slot.set(Arc::clone(&service));
        let start = Instant::now();
        std::thread::scope(|scope| {
            for thread in 0..4u64 {
                let service = &service;
                scope.spawn(move || {
                    for n in 0..200u64 {
                        let ms = thread * 1_000 + n;
                        service.handle_at(
                            report_with(json!({ "positionMs": ms })),
                            start + Duration::from_secs(ms),
                        );
                    }
                });
            }
        });
        assert_eq!(*lock(&mismatches), 0);
        assert_eq!(lock(&emitted).last(), Some(&service.state()));
    }

    #[test]
    fn missing_buttons_report_unavailable() {
        let f = fixture();
        f.service.handle(report_with(json!({})));
        // The bridge lost the bar or a button while the video plays on.
        f.service
            .handle(report_with(json!({ "available": false, "playing": true })));
        let state = f.service.state();
        assert_eq!(state.status, YtmStatus::Unavailable);
        assert!(!state.is_playing());
        assert_eq!(
            lock(&f.followed).last().unwrap().status,
            YtmStatus::Unavailable
        );
        for _ in 0..5 {
            f.service.handle(report_with(json!({
                "available": false, "playing": false, "canNext": false, "canPrev": false,
            })));
        }
        assert_eq!(f.service.state().status, YtmStatus::Unavailable);
        // No eval loop: reports never make the service talk to the page.
        assert!(lock(&f.page.0).is_empty());

        // A transport following the service hands control back to Prism.
        use crate::shell::transport::{Active, Clock, PrismControl, Transport};
        use std::time::Instant;
        struct Quiet;
        impl PrismControl for Quiet {
            fn toggle(&self) -> Result<(), String> {
                Ok(())
            }
            fn pause(&self) -> Result<(), String> {
                Ok(())
            }
            fn next(&self) -> Result<(), String> {
                Ok(())
            }
            fn prev(&self) -> Result<(), String> {
                Ok(())
            }
            fn seek(&self, _: u64) -> Result<(), String> {
                Ok(())
            }
            fn set_volume(&self, _: f32) -> Result<(), String> {
                Ok(())
            }
        }
        struct Now;
        impl Clock for Now {
            fn now(&self) -> Instant {
                Instant::now()
            }
        }
        let service = Arc::new(fixture().service);
        let transport = Arc::new(Transport::with_control(
            Arc::new(Quiet),
            Arc::clone(&service) as Arc<dyn YtmControl>,
            Arc::new(Now),
        ));
        let weak = Arc::downgrade(&transport);
        service.follow(Arc::new(move |state| {
            if let Some(transport) = weak.upgrade() {
                transport.on_ytm_state(state);
            }
        }));
        service.handle(report_with(json!({})));
        assert_eq!(transport.active(), Active::Ytm);
        service.handle(report_with(json!({ "available": false })));
        assert_eq!(transport.active(), Active::Prism);
    }

    #[test]
    fn eval_snippets_are_constants() {
        let names: Vec<&str> = SNIPPETS.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            ["toggle", "play", "pause", "next", "prev", "seek", "volume"]
        );
        for (name, script) in SNIPPETS {
            assert_eq!(
                script,
                format!(
                    "window.dispatchEvent(new CustomEvent('prism-ytm', {{ detail: '{name}' }}));"
                )
            );
        }
        let fixed: Vec<&str> = SNIPPETS.iter().map(|(_, script)| *script).collect();
        let is_number = |text: &str| {
            !text.is_empty()
                && text.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
                && text.chars().filter(|ch| *ch == '.').count() <= 1
                && !text.starts_with('.')
                && !text.ends_with('.')
        };
        let check = |action: YtmAction| -> Vec<String> {
            let sent = scripts(action);
            for script in &sent {
                if fixed.contains(&script.as_str()) {
                    continue;
                }
                let number = script
                    .strip_prefix("window.__prismSeekMs = ")
                    .or_else(|| script.strip_prefix("window.__prismVolume = "))
                    .and_then(|rest| rest.strip_suffix(';'))
                    .unwrap_or_else(|| panic!("not a fixed snippet: {script}"));
                assert!(is_number(number), "not a number literal: {script}");
            }
            sent
        };

        for action in [
            YtmAction::Toggle,
            YtmAction::Play,
            YtmAction::Pause,
            YtmAction::Next,
            YtmAction::Prev,
        ] {
            assert_eq!(check(action).len(), 1);
        }
        assert_eq!(
            check(YtmAction::Seek(61_500)),
            ["window.__prismSeekMs = 61500;", snippet("seek")]
        );
        assert_eq!(
            check(YtmAction::Seek(u64::MAX))[0],
            format!("window.__prismSeekMs = {};", report::MAX_MS)
        );
        assert_eq!(
            check(YtmAction::Volume(0.5)),
            ["window.__prismVolume = 0.500;", snippet("volume")]
        );
        assert_eq!(
            check(YtmAction::Volume(7.0))[0],
            "window.__prismVolume = 1.000;"
        );
        assert_eq!(
            check(YtmAction::Volume(-0.0))[0],
            "window.__prismVolume = 0.000;"
        );
        assert_eq!(
            check(YtmAction::Volume(-3.0))[0],
            "window.__prismVolume = 0.000;"
        );
        assert_eq!(
            check(YtmAction::Volume(1e-9))[0],
            "window.__prismVolume = 0.000;"
        );
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(check(YtmAction::Volume(bad)).is_empty());
        }

        // The service sends exactly these, in order.
        let f = fixture();
        f.service.send(YtmAction::Seek(1_000));
        f.service.send(YtmAction::Volume(f32::NAN));
        f.service.send(YtmAction::Pause);
        assert_eq!(
            *lock(&f.page.0),
            [
                "window.__prismSeekMs = 1000;",
                snippet("seek"),
                snippet("pause")
            ]
        );
    }

    #[test]
    fn control_actions_parse() {
        assert_eq!(parse_action("toggle", None, None), Ok(YtmAction::Toggle));
        assert_eq!(parse_action("prev", Some(5.0), None), Ok(YtmAction::Prev));
        assert_eq!(
            parse_action("seek", Some(1234.4), None),
            Ok(YtmAction::Seek(1234))
        );
        assert_eq!(
            parse_action("seek", Some(1e300), None),
            Ok(YtmAction::Seek(report::MAX_MS))
        );
        assert!(parse_action("seek", None, None).is_err());
        assert!(parse_action("seek", Some(-1.0), None).is_err());
        assert!(parse_action("seek", Some(f64::NAN), None).is_err());
        assert_eq!(
            parse_action("volume", None, Some(0.25)),
            Ok(YtmAction::Volume(0.25))
        );
        assert_eq!(
            parse_action("volume", None, Some(3.0)),
            Ok(YtmAction::Volume(1.0))
        );
        assert!(parse_action("volume", None, Some(f64::INFINITY)).is_err());
        assert!(parse_action("volume", None, None).is_err());
        assert!(parse_action("Toggle", None, None).is_err());
        assert!(parse_action("eval", None, None).is_err());
    }

    #[test]
    fn report_from_another_webview_is_refused() {
        let url = |raw: &str| tauri::Url::parse(raw).unwrap();
        let page = url("https://music.youtube.com/watch?v=abc");
        assert!(report_allowed("ytm", Some(&page), false));
        assert!(report_allowed("ytm", Some(&page), true));
        // Other webviews, even on the right page.
        for label in ["main", "mini", "ytm-signin", "YTM", ""] {
            assert!(!report_allowed(label, Some(&page), false), "{label}");
        }
        // The right view on another page.
        for other in [
            "http://music.youtube.com/",
            "https://music.youtube.com:8443/",
            "https://www.youtube.com/",
            "https://accounts.google.com/",
            "https://music.youtube.com.evil.example/",
            "tauri://localhost/",
            "http://localhost:1440/",
            "http://127.0.0.1:4321/",
            "about:blank",
        ] {
            assert!(!report_allowed("ytm", Some(&url(other)), false), "{other}");
        }
        assert!(!report_allowed("ytm", None, false));
        // The fake page only with the dev flag, and only over http on 127.0.0.1.
        assert!(report_allowed(
            "ytm",
            Some(&url("http://127.0.0.1:4321/")),
            true
        ));
        assert!(!report_allowed(
            "ytm",
            Some(&url("http://localhost:4321/")),
            true
        ));
        assert!(!report_allowed(
            "main",
            Some(&url("http://127.0.0.1:4321/")),
            true
        ));
        #[cfg(not(debug_assertions))]
        assert!(!dev_host());
    }

    #[test]
    fn bridge_is_wrapped_with_the_dev_flag() {
        for flag in [true, false] {
            let script = bridge_script(flag);
            assert!(script.starts_with(&format!("{{ const ALLOW_DEV_HOST = {flag};\n")));
            assert!(script.ends_with("\n}"));
            assert!(script.contains(BRIDGE));
        }
    }
}
