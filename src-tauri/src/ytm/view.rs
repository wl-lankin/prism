//! The YouTube Music view: a child web view (`ytm`) in the main window, over the page's
//! placeholder. Created the first time the user opens YouTube Music, hidden (never
//! destroyed) when the user leaves it, while an overlay is open and while the main window is
//! closed; destroyed only on sign-out and on quit.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Manager, Url};

use super::policy::{self, Decision, From};
use super::YtmService;

pub const PAGE_URL: &str = "https://music.youtube.com/";
/// Under Prism's data directory (Windows only): the view's own WebView2 profile, shared
/// with the sign-in window.
pub const DATA_DIR_NAME: &str = "ytm-webview";
/// WebView2 neither takes the media keys nor shows its own media overlay entry. The
/// switches wry sets by default are repeated, because only the last `--disable-features`
/// counts.
pub const BROWSER_ARGS: &str =
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,HardwareMediaKeyHandling";
/// A hand-off to the system browser or the sign-in window at most this often.
pub const HAND_OFF_SPACING: Duration = Duration::from_secs(1);
/// Windows: the data directory is removed again this long after a first failure.
pub const REMOVE_RETRY: Duration = Duration::from_millis(500);

/// Where the view goes in the main window, in logical pixels (the placeholder's rect).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The rect inside the main window's inner size (logical px): never negative, never past the
/// right or bottom edge; a value that is not a number counts as 0.
pub fn clamp(rect: Rect, inner: Option<(f64, f64)>) -> Rect {
    let finite = |value: f64| {
        if value.is_finite() {
            value.max(0.0)
        } else {
            0.0
        }
    };
    let (mut x, mut y, mut width, mut height) = (
        finite(rect.x),
        finite(rect.y),
        finite(rect.width),
        finite(rect.height),
    );
    if let Some((inner_width, inner_height)) = inner {
        let (inner_width, inner_height) = (finite(inner_width), finite(inner_height));
        x = x.min(inner_width);
        y = y.min(inner_height);
        width = width.min(inner_width - x);
        height = height.min(inner_height - y);
    }
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// The native side of the view, so the rules here run without an app in tests.
pub trait ViewHost: Send + Sync {
    fn exists(&self) -> bool;
    /// The main window's inner size in logical pixels.
    fn main_size(&self) -> Option<(f64, f64)>;
    /// Creates the view, shown, at `rect`. Must not run on the main thread.
    fn create(&self, rect: Rect) -> Result<(), String>;
    fn set_bounds(&self, rect: Rect) -> Result<(), String>;
    fn show(&self) -> Result<(), String>;
    fn hide(&self) -> Result<(), String>;
    fn close(&self) -> Result<(), String>;
    /// Clears the cookies and storage the view's session lives in.
    fn clear_data(&self) -> Result<(), String>;
    /// Opens the sign-in window at the standard page (or focuses it), without waiting.
    fn open_sign_in(&self);
    fn close_sign_in(&self);
    /// Removes the view's own data directory (Windows); nothing to do elsewhere.
    fn remove_data_dir(&self) -> Result<(), String>;
    /// Runs `job` off the calling thread (the view is created there).
    fn spawn(&self, job: Box<dyn FnOnce() + Send>);
}

#[derive(Default)]
struct Inner {
    /// The page wants the view shown (its placeholder is up and no overlay is open).
    shown: bool,
    rect: Option<Rect>,
    main_closed: bool,
    creating: bool,
    last_hand_off: Option<Instant>,
}

pub struct YtmView {
    host: Arc<dyn ViewHost>,
    service: Arc<YtmService>,
    inner: Mutex<Inner>,
    /// Held while the view is created or signed out; never taken on the main thread.
    create: Mutex<()>,
    remove_retry: Duration,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn log(what: &str, result: Result<(), String>) {
    if let Err(err) = result {
        eprintln!("{what} failed: {err}");
    }
}

impl YtmView {
    pub fn new(host: Arc<dyn ViewHost>, service: Arc<YtmService>) -> YtmView {
        YtmView::with_retry(host, service, REMOVE_RETRY)
    }

    fn with_retry(
        host: Arc<dyn ViewHost>,
        service: Arc<YtmService>,
        remove_retry: Duration,
    ) -> YtmView {
        YtmView {
            host,
            service,
            inner: Mutex::new(Inner::default()),
            create: Mutex::new(()),
            remove_retry,
        }
    }

    pub fn is_shown(&self) -> bool {
        lock(&self.inner).shown
    }

    /// Whether the user can see the view right now.
    pub fn is_visible(&self) -> bool {
        let inner = lock(&self.inner);
        inner.shown && !inner.main_closed
    }

    /// `ytm_show`: puts the view at `rect` and shows it, creating it first if needed (in the
    /// background: the call returns at once).
    pub fn show(self: &Arc<Self>, rect: Rect) -> Result<(), String> {
        let rect = clamp(rect, self.host.main_size());
        let (main_closed, create) = {
            let mut inner = lock(&self.inner);
            inner.shown = true;
            inner.rect = Some(rect);
            let create = !self.host.exists() && !inner.creating;
            if create {
                inner.creating = true;
            }
            (inner.main_closed, create)
        };
        if create {
            let view = Arc::clone(self);
            self.host.spawn(Box::new(move || view.create_now()));
            return Ok(());
        }
        if self.host.exists() {
            self.host.set_bounds(rect)?;
            if !main_closed {
                self.host.show()?;
            }
        }
        Ok(())
    }

    fn create_now(&self) {
        let _create = lock(&self.create);
        let rect = lock(&self.inner).rect;
        let created = match rect {
            Some(rect) if !self.host.exists() => self.host.create(rect),
            _ => Ok(()),
        };
        let (visible, latest) = {
            let mut inner = lock(&self.inner);
            inner.creating = false;
            (inner.shown && !inner.main_closed, inner.rect)
        };
        if let Err(err) = created {
            eprintln!("creating the YouTube Music view failed: {err}");
            return;
        }
        // Hidden or moved while it was being created.
        if latest != rect {
            if let Some(latest) = latest {
                log(
                    "placing the YouTube Music view",
                    self.host.set_bounds(latest),
                );
            }
        }
        if !visible {
            log("hiding the YouTube Music view", self.host.hide());
        }
    }

    /// `ytm_hide`: the user left the view or an overlay opened. The page keeps playing.
    pub fn hide(&self) -> Result<(), String> {
        lock(&self.inner).shown = false;
        if self.host.exists() {
            self.host.hide()?;
        }
        Ok(())
    }

    /// The main window closed to the mini player: the view goes with it and keeps playing.
    pub fn on_main_close(&self) {
        lock(&self.inner).main_closed = true;
        if self.host.exists() {
            log("hiding the YouTube Music view", self.host.hide());
        }
    }

    /// The main window is back: the view returns where it was, if the page still wants it.
    pub fn on_main_show(&self) {
        let shown = {
            let mut inner = lock(&self.inner);
            inner.main_closed = false;
            inner.shown
        };
        if shown && self.host.exists() {
            log("showing the YouTube Music view", self.host.show());
        }
    }

    /// Quit: the view goes, and control falls back to Prism.
    pub fn destroy(&self) {
        self.forget();
        self.service.reset();
    }

    fn forget(&self) {
        {
            let mut inner = lock(&self.inner);
            inner.shown = false;
            inner.rect = None;
        }
        if self.host.exists() {
            log("closing the YouTube Music view", self.host.close());
        }
    }

    /// Settings' "Sign in".
    pub fn sign_in(&self) {
        self.host.open_sign_in();
    }

    /// Sign-out: the session's data is cleared, the sign-in window and the view go, the
    /// state is idle again, and (Windows) the view's data directory is removed once the
    /// view is gone. Blocks for up to [`REMOVE_RETRY`]; not for the main thread.
    pub fn sign_out(&self) {
        let _create = lock(&self.create);
        log("clearing the YouTube Music session", self.host.clear_data());
        self.host.close_sign_in();
        self.forget();
        self.service.reset();
        if let Err(err) = self.host.remove_data_dir() {
            std::thread::sleep(self.remove_retry);
            if let Err(again) = self.host.remove_data_dir() {
                eprintln!("removing the YouTube Music data failed: {err}; again: {again}");
            }
        }
    }

    /// Whether a navigation may open the sign-in window or the system browser now: only
    /// while the view is visible for the view (the user clicked something), and at most once
    /// per [`HAND_OFF_SPACING`].
    pub fn may_hand_off(&self, from: From, now: Instant) -> bool {
        let mut inner = lock(&self.inner);
        if from == From::View && !(inner.shown && !inner.main_closed) {
            return false;
        }
        if inner
            .last_hand_off
            .is_some_and(|last| now.saturating_duration_since(last) < HAND_OFF_SPACING)
        {
            return false;
        }
        inner.last_hand_off = Some(now);
        true
    }
}

fn view_state(app: &AppHandle) -> Option<Arc<YtmView>> {
    app.try_state::<crate::AppState>()
        .map(|state| Arc::clone(&state.ytm_view))
}

fn may_hand_off(app: &AppHandle, from: From) -> bool {
    view_state(app).is_some_and(|view| view.may_hand_off(from, Instant::now()))
}

/// Opens an https page in the system browser (`decide` already refused everything else).
pub(crate) fn open_outside(app: &AppHandle, url: &Url) {
    use tauri_plugin_opener::OpenerExt;
    if url.scheme() != "https" {
        return;
    }
    let (app, url) = (app.clone(), url.to_string());
    tauri::async_runtime::spawn(async move {
        if let Err(err) = app.opener().open_url(url, None::<&str>) {
            eprintln!("opening a link from YouTube Music failed: {err}");
        }
    });
}

/// The view's navigation and new-window decisions, with their effects. Runs on the main
/// thread inside the web view's callback, so the effects run later, elsewhere.
fn act(app: &AppHandle, data_dir: &Path, url: &Url) -> bool {
    match policy::decide(url, From::View) {
        Decision::Stay => true,
        Decision::SignIn => {
            if may_hand_off(app, From::View) {
                super::signin::open_later(app, data_dir, Some(url.clone()));
            }
            false
        }
        Decision::External => {
            if may_hand_off(app, From::View) {
                open_outside(app, url);
            }
            false
        }
        Decision::Block => false,
    }
}

/// The child view on the main window.
pub struct TauriHost {
    app: AppHandle,
    /// Prism's data directory.
    data_dir: PathBuf,
}

impl TauriHost {
    pub fn new(app: AppHandle, data_dir: PathBuf) -> TauriHost {
        TauriHost { app, data_dir }
    }

    fn webview(&self) -> Option<tauri::Webview> {
        self.app.get_webview(super::VIEW_LABEL)
    }

    fn with_webview(
        &self,
        f: impl FnOnce(&tauri::Webview) -> tauri::Result<()>,
    ) -> Result<(), String> {
        match self.webview() {
            Some(view) => f(&view).map_err(|err| err.to_string()),
            None => Ok(()),
        }
    }

    fn builder(&self) -> Result<tauri::webview::WebviewBuilder<tauri::Wry>, String> {
        use tauri::utils::config::BackgroundThrottlingPolicy;
        use tauri::webview::{NewWindowResponse, WebviewBuilder};
        let url = Url::parse(PAGE_URL).map_err(|err| err.to_string())?;
        let on_navigation = {
            let (app, data_dir) = (self.app.clone(), self.data_dir.clone());
            move |url: &Url| act(&app, &data_dir, url)
        };
        let on_new_window = {
            let (app, data_dir) = (self.app.clone(), self.data_dir.clone());
            move |url: Url, _features| {
                // A YouTube Music page in a new window has no place to go; the rest is
                // handed off as a navigation would be.
                if policy::decide(&url, From::View) != Decision::Stay {
                    act(&app, &data_dir, &url);
                }
                NewWindowResponse::Deny
            }
        };
        let builder = WebviewBuilder::new(super::VIEW_LABEL, tauri::WebviewUrl::External(url))
            .initialization_script(super::bridge_script(super::dev_host()))
            .background_throttling(BackgroundThrottlingPolicy::Disabled)
            .on_navigation(on_navigation)
            .on_new_window(on_new_window);
        // `cfg!` rather than `#[cfg]`, so these calls are type-checked on macOS too.
        Ok(if cfg!(windows) {
            builder
                .data_directory(self.data_dir.join(DATA_DIR_NAME))
                .additional_browser_args(BROWSER_ARGS)
        } else {
            builder
        })
    }
}

impl ViewHost for TauriHost {
    fn exists(&self) -> bool {
        self.webview().is_some()
    }

    fn main_size(&self) -> Option<(f64, f64)> {
        let window = self.app.get_window(crate::shell::MAIN_LABEL)?;
        let scale = window.scale_factor().ok()?;
        let size = window.inner_size().ok()?.to_logical::<f64>(scale);
        Some((size.width, size.height))
    }

    fn create(&self, rect: Rect) -> Result<(), String> {
        let window = self
            .app
            .get_window(crate::shell::MAIN_LABEL)
            .ok_or("the main window is gone")?;
        window
            .add_child(
                self.builder()?,
                tauri::LogicalPosition::new(rect.x, rect.y),
                tauri::LogicalSize::new(rect.width, rect.height),
            )
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    fn set_bounds(&self, rect: Rect) -> Result<(), String> {
        self.with_webview(|view| {
            view.set_bounds(tauri::Rect {
                position: tauri::LogicalPosition::new(rect.x, rect.y).into(),
                size: tauri::LogicalSize::new(rect.width, rect.height).into(),
            })
        })
    }

    fn show(&self) -> Result<(), String> {
        self.with_webview(|view| view.show())
    }

    fn hide(&self) -> Result<(), String> {
        self.with_webview(|view| view.hide())
    }

    fn close(&self) -> Result<(), String> {
        self.with_webview(|view| view.close())
    }

    fn clear_data(&self) -> Result<(), String> {
        let view = self
            .webview()
            .or_else(|| self.app.get_webview(super::signin::SIGNIN_LABEL));
        // macOS: the view uses the default data store, which the main page shares.
        let view = if cfg!(windows) {
            view
        } else {
            view.or_else(|| self.app.get_webview(crate::shell::MAIN_LABEL))
        };
        match view {
            Some(view) => view
                .clear_all_browsing_data()
                .map_err(|err| err.to_string()),
            None => Ok(()),
        }
    }

    fn open_sign_in(&self) {
        super::signin::open_later(&self.app, &self.data_dir, None);
    }

    fn close_sign_in(&self) {
        super::signin::close(&self.app);
    }

    fn remove_data_dir(&self) -> Result<(), String> {
        if !cfg!(windows) {
            return Ok(());
        }
        let dir = self.data_dir.join(DATA_DIR_NAME);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("{}: {err}", dir.display())),
        }
    }

    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        tauri::async_runtime::spawn_blocking(job);
    }
}

/// Set once the sign-in window reached YouTube Music, so the view reloads only once.
pub(crate) type Done = Arc<AtomicBool>;

/// The sign-in window finished: it closes and the view reloads with the new session.
pub(crate) fn finish_sign_in(app: &AppHandle, done: &Done) {
    if done.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        super::signin::close(&app);
        if let Some(view) = app.get_webview(super::VIEW_LABEL) {
            if let Err(err) = view.reload() {
                eprintln!("reloading YouTube Music after the sign-in failed: {err}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::transport::{Active, Clock, PrismControl, Transport};
    use crate::ytm::report::{self, tests::bridge_report};
    use crate::ytm::state::{YtmControl, YtmState, YtmStatus};
    use crate::ytm::PageEval;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct FakeHost {
        exists: AtomicBool,
        size: Mutex<Option<(f64, f64)>>,
        ops: Mutex<Vec<String>>,
        remove_failures: AtomicUsize,
    }

    impl FakeHost {
        fn record(&self, op: impl Into<String>) {
            lock(&self.ops).push(op.into());
        }

        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *lock(&self.ops))
        }
    }

    impl ViewHost for FakeHost {
        fn exists(&self) -> bool {
            self.exists.load(Ordering::SeqCst)
        }
        fn main_size(&self) -> Option<(f64, f64)> {
            *lock(&self.size)
        }
        fn create(&self, rect: Rect) -> Result<(), String> {
            self.exists.store(true, Ordering::SeqCst);
            self.record(format!(
                "create {} {} {} {}",
                rect.x, rect.y, rect.width, rect.height
            ));
            Ok(())
        }
        fn set_bounds(&self, rect: Rect) -> Result<(), String> {
            self.record(format!(
                "bounds {} {} {} {}",
                rect.x, rect.y, rect.width, rect.height
            ));
            Ok(())
        }
        fn show(&self) -> Result<(), String> {
            self.record("show");
            Ok(())
        }
        fn hide(&self) -> Result<(), String> {
            self.record("hide");
            Ok(())
        }
        fn close(&self) -> Result<(), String> {
            self.exists.store(false, Ordering::SeqCst);
            self.record("close");
            Ok(())
        }
        fn clear_data(&self) -> Result<(), String> {
            self.record("clear");
            Ok(())
        }
        fn open_sign_in(&self) {
            self.record("open sign-in");
        }
        fn close_sign_in(&self) {
            self.record("close sign-in");
        }
        fn remove_data_dir(&self) -> Result<(), String> {
            self.record("remove");
            let failures = self.remove_failures.load(Ordering::SeqCst);
            if failures > 0 {
                self.remove_failures.store(failures - 1, Ordering::SeqCst);
                return Err("in use".into());
            }
            Ok(())
        }
        fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
            self.record("spawn");
            job();
        }
    }

    struct NoPage;
    impl PageEval for NoPage {
        fn eval(&self, _: &str) {}
    }

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

    struct Fixture {
        host: Arc<FakeHost>,
        service: Arc<YtmService>,
        transport: Arc<Transport>,
        emitted: Arc<Mutex<Vec<YtmState>>>,
        view: Arc<YtmView>,
    }

    fn fixture() -> Fixture {
        let host = Arc::new(FakeHost::default());
        *lock(&host.size) = Some((1280.0, 820.0));
        let emitted: Arc<Mutex<Vec<YtmState>>> = Arc::default();
        let service = Arc::new(YtmService::new(Arc::new(NoPage), {
            let emitted = Arc::clone(&emitted);
            Arc::new(move |state| lock(&emitted).push(state.clone()))
        }));
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
        let view = Arc::new(YtmView::with_retry(
            Arc::clone(&host) as Arc<dyn ViewHost>,
            Arc::clone(&service),
            Duration::ZERO,
        ));
        Fixture {
            host,
            service,
            transport,
            emitted,
            view,
        }
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn play(f: &Fixture) {
        f.service.handle(report::parse(bridge_report()).unwrap());
        assert_eq!(f.service.state().status, YtmStatus::Playing);
        assert_eq!(f.transport.active(), Active::Ytm);
    }

    #[test]
    fn bounds_are_clamped() {
        let inner = Some((1280.0, 820.0));
        // Inside: unchanged.
        assert_eq!(
            clamp(rect(232.0, 48.0, 1048.0, 680.0), inner),
            rect(232.0, 48.0, 1048.0, 680.0)
        );
        // Past the right and bottom edges.
        assert_eq!(
            clamp(rect(232.0, 48.0, 5000.0, 5000.0), inner),
            rect(232.0, 48.0, 1048.0, 772.0)
        );
        // Negative and off the window.
        assert_eq!(
            clamp(rect(-20.0, -5.0, 100.0, 50.0), inner),
            rect(0.0, 0.0, 100.0, 50.0)
        );
        assert_eq!(
            clamp(rect(2000.0, 900.0, 100.0, 50.0), inner),
            rect(1280.0, 820.0, 0.0, 0.0)
        );
        assert_eq!(
            clamp(rect(10.0, 10.0, -100.0, -1.0), inner),
            rect(10.0, 10.0, 0.0, 0.0)
        );
        // Not a number.
        assert_eq!(
            clamp(
                rect(f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::NAN),
                inner
            ),
            rect(0.0, 0.0, 0.0, 0.0)
        );
        // Without a known window size only the sign and finiteness are fixed.
        assert_eq!(
            clamp(rect(-1.0, 4.0, 9000.0, f64::NAN), None),
            rect(0.0, 4.0, 9000.0, 0.0)
        );

        // The view gets the clamped rect, on creation and after.
        let f = fixture();
        f.view.show(rect(232.0, 48.0, 5000.0, 680.0)).unwrap();
        assert_eq!(f.host.take(), ["spawn", "create 232 48 1048 680"]);
        *lock(&f.host.size) = Some((1000.0, 700.0));
        f.view.show(rect(232.0, 48.0, 1048.0, 680.0)).unwrap();
        assert_eq!(f.host.take(), ["bounds 232 48 768 652", "show"]);
    }

    #[test]
    fn hide_keeps_the_webview() {
        let f = fixture();
        f.view.show(rect(232.0, 48.0, 1048.0, 680.0)).unwrap();
        play(&f);
        f.host.take();

        f.view.hide().unwrap();
        assert!(!f.view.is_shown());
        assert!(f.host.exists());
        assert_eq!(f.host.take(), ["hide"]);
        assert_eq!(f.service.state().status, YtmStatus::Playing);
        assert_eq!(f.transport.active(), Active::Ytm);

        // Coming back moves and shows the same view; nothing new is created.
        f.view.show(rect(232.0, 48.0, 1000.0, 600.0)).unwrap();
        assert!(f.view.is_shown());
        assert_eq!(f.host.take(), ["bounds 232 48 1000 600", "show"]);

        // A hide before the first show has nothing to hide.
        let g = fixture();
        g.view.hide().unwrap();
        assert!(g.host.take().is_empty());
    }

    #[test]
    fn hidden_while_created_stays_hidden() {
        struct Later {
            host: FakeHost,
            job: Mutex<Option<Box<dyn FnOnce() + Send>>>,
        }
        impl ViewHost for Later {
            fn exists(&self) -> bool {
                self.host.exists()
            }
            fn main_size(&self) -> Option<(f64, f64)> {
                None
            }
            fn create(&self, rect: Rect) -> Result<(), String> {
                self.host.create(rect)
            }
            fn set_bounds(&self, rect: Rect) -> Result<(), String> {
                self.host.set_bounds(rect)
            }
            fn show(&self) -> Result<(), String> {
                self.host.show()
            }
            fn hide(&self) -> Result<(), String> {
                self.host.hide()
            }
            fn close(&self) -> Result<(), String> {
                self.host.close()
            }
            fn clear_data(&self) -> Result<(), String> {
                self.host.clear_data()
            }
            fn open_sign_in(&self) {}
            fn close_sign_in(&self) {}
            fn remove_data_dir(&self) -> Result<(), String> {
                Ok(())
            }
            fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
                *lock(&self.job) = Some(job);
            }
        }
        let host = Arc::new(Later {
            host: FakeHost::default(),
            job: Mutex::new(None),
        });
        let service = Arc::new(YtmService::new(Arc::new(NoPage), Arc::new(|_| {})));
        let view = Arc::new(YtmView::new(
            Arc::clone(&host) as Arc<dyn ViewHost>,
            service,
        ));
        view.show(rect(1.0, 2.0, 3.0, 4.0)).unwrap();
        // A second show while the first creation runs creates nothing more.
        view.show(rect(5.0, 6.0, 7.0, 8.0)).unwrap();
        view.hide().unwrap();
        assert!(host.host.take().is_empty());
        let job = lock(&host.job).take().unwrap();
        job();
        assert_eq!(
            host.host.take(),
            ["create 5 6 7 8", "hide"],
            "created at the latest rect, then hidden as the page asked"
        );
    }

    #[test]
    fn main_close_keeps_ytm_playing() {
        let f = fixture();
        f.view.show(rect(232.0, 48.0, 1048.0, 680.0)).unwrap();
        play(&f);
        f.host.take();

        f.view.on_main_close();
        assert_eq!(f.host.take(), ["hide"]);
        assert!(f.host.exists());
        assert!(!f.view.is_visible());
        assert_eq!(f.service.state().status, YtmStatus::Playing);
        assert_eq!(f.transport.active(), Active::Ytm);

        // A rect from the page while closed moves the view but does not show it.
        f.view.show(rect(232.0, 48.0, 1000.0, 680.0)).unwrap();
        assert_eq!(f.host.take(), ["bounds 232 48 1000 680"]);

        // Back from the mini player: shown again, at the same place.
        f.view.on_main_show();
        assert_eq!(f.host.take(), ["show"]);
        assert!(f.view.is_visible());

        // The view was left before closing: it stays hidden on the way back.
        f.view.hide().unwrap();
        f.view.on_main_close();
        f.view.on_main_show();
        assert_eq!(f.host.take(), ["hide", "hide"]);
        assert_eq!(f.service.state().status, YtmStatus::Playing);
    }

    #[test]
    fn sign_out_forgets_the_view() {
        let f = fixture();
        f.view.show(rect(232.0, 48.0, 1048.0, 680.0)).unwrap();
        play(&f);
        f.host.take();
        f.emitted.lock().unwrap().clear();

        f.view.sign_out();
        assert_eq!(f.host.take(), ["clear", "close sign-in", "close", "remove"]);
        assert!(!f.host.exists());
        assert!(!f.view.is_shown());
        assert_eq!(f.service.state(), YtmState::default());
        assert_eq!(*lock(&f.emitted), [YtmState::default()]);
        assert_eq!(f.transport.active(), Active::Prism);

        // The next visit creates a fresh view.
        f.view.show(rect(232.0, 48.0, 1048.0, 680.0)).unwrap();
        assert_eq!(f.host.take(), ["spawn", "create 232 48 1048 680"]);

        // A data directory still in use is tried once more, then left.
        f.host.remove_failures.store(5, Ordering::SeqCst);
        f.view.sign_out();
        assert_eq!(
            f.host.take(),
            ["clear", "close sign-in", "close", "remove", "remove"]
        );
        f.host.remove_failures.store(1, Ordering::SeqCst);
        f.view.sign_out();
        assert_eq!(
            f.host.take(),
            ["clear", "close sign-in", "remove", "remove"]
        );
    }

    #[test]
    fn quit_destroys_the_view() {
        let f = fixture();
        f.view.show(rect(232.0, 48.0, 1048.0, 680.0)).unwrap();
        play(&f);
        f.host.take();
        f.view.destroy();
        assert_eq!(f.host.take(), ["close"]);
        assert_eq!(f.service.state(), YtmState::default());
        assert_eq!(f.transport.active(), Active::Prism);
        // Nothing to close the second time.
        f.view.destroy();
        assert!(f.host.take().is_empty());
        // Settings' "Sign in" needs no view.
        f.view.sign_in();
        assert_eq!(f.host.take(), ["open sign-in"]);
    }

    #[test]
    fn hand_offs_need_a_visible_view_and_are_spaced() {
        let f = fixture();
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        // A hidden view (a frame loading in the background) never opens anything.
        assert!(!f.view.may_hand_off(From::View, at(0)));
        f.view.show(rect(0.0, 0.0, 10.0, 10.0)).unwrap();
        assert!(f.view.may_hand_off(From::View, at(0)));
        assert!(!f.view.may_hand_off(From::View, at(500)));
        assert!(!f.view.may_hand_off(From::SignIn, at(900)));
        assert!(f.view.may_hand_off(From::View, at(1000)));
        f.view.on_main_close();
        assert!(!f.view.may_hand_off(From::View, at(5000)));
        // The sign-in window has its own window; only the spacing counts for it.
        assert!(f.view.may_hand_off(From::SignIn, at(5000)));
        f.view.on_main_show();
        f.view.hide().unwrap();
        assert!(!f.view.may_hand_off(From::View, at(9000)));
        assert!(f.view.may_hand_off(From::SignIn, at(9000)));
    }

    #[test]
    fn windows_profile_and_switches() {
        assert_eq!(DATA_DIR_NAME, "ytm-webview");
        let features = BROWSER_ARGS
            .strip_prefix("--disable-features=")
            .expect("one --disable-features switch");
        assert!(!features.contains(' '));
        assert!(features.split(',').any(|f| f == "HardwareMediaKeyHandling"));
        assert_eq!(
            Url::parse(PAGE_URL).unwrap().host_str(),
            Some("music.youtube.com")
        );
        assert_eq!(
            policy::decide(&Url::parse(PAGE_URL).unwrap(), From::View),
            Decision::Stay
        );
    }
}
