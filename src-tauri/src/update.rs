//! Signed updates from the repo's GitHub releases.
//!
//! The first check runs a minute after start, then every 6 hours, and only when online. A
//! found update downloads in the background (with "Install updates automatically" off it is
//! only announced). A downloaded update is installed on quit or when the user presses
//! "Restart to update", never while Prism keeps playing. The plugin checks the signature
//! against the public key in `tauri.conf.json` on download and again on install, so a
//! failure leaves the installed version as it is; the next check tries again.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Sent to the main window with the full [`UpdateState`] after every change.
pub const EVENT_STATE: &str = "update://state";

/// The only place updates come from.
pub const ENDPOINT: &str =
    "https://github.com/wl-lankin/prism/releases/latest/download/latest.json";

pub const FIRST_CHECK: Duration = Duration::from_secs(60);
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// After a scheduled check that was skipped (offline) or failed.
pub const RETRY_AFTER: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    /// Found while "Install updates automatically" is off: waits for the user.
    Available,
    Downloading,
    /// Downloaded and verified; installs on quit or on "Restart to update".
    Ready,
    Failed,
    UpToDate,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    pub status: UpdateStatus,
    /// The version found.
    pub version: Option<String>,
    /// Its release notes from `latest.json`.
    pub notes: Option<String>,
    /// 0..=1 while downloading, when the size is known.
    pub progress: Option<f32>,
    /// When the last check finished, in ms since the epoch.
    pub last_check: Option<i64>,
    /// Quitting installs the ready update (else only "Restart to update" does).
    pub installs_on_quit: bool,
}

/// An update the checker found.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub version: String,
    pub notes: Option<String>,
}

/// Where updates come from and how they are put in place.
pub trait Checker: Send + Sync {
    /// Whether a check could reach anything. A scheduled check is skipped when not.
    fn online(&self) -> bool;
    fn check(&self) -> Result<Option<Found>, String>;
    /// Downloads and verifies the last found update. `progress` gets 0..=1 when the size is
    /// known.
    fn download(&self, progress: &mut dyn FnMut(f32)) -> Result<(), String>;
    /// Installs the downloaded update. On Windows this ends the process; `relaunch` says
    /// whether the installer starts Prism again.
    fn install(&self, relaunch: bool) -> Result<(), String>;
}

pub type StateEmit = Arc<dyn Fn(&UpdateState) + Send + Sync>;
pub type AutoFlag = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Debug, Clone, Copy)]
pub struct UpdateOptions {
    /// Off in dev builds: they never check on their own.
    pub schedule: bool,
    pub first: Duration,
    pub every: Duration,
    pub retry: Duration,
}

impl Default for UpdateOptions {
    fn default() -> Self {
        UpdateOptions {
            schedule: true,
            first: FIRST_CHECK,
            every: CHECK_EVERY,
            retry: RETRY_AFTER,
        }
    }
}

/// How a check went, for when the next scheduled one runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Done,
    /// Skipped while offline, or the check or the download failed.
    RetrySoon,
}

enum Wake {
    /// The user asked: checks even when the probe says offline, and downloads what it finds.
    Check,
    /// "Install updates automatically" changed: fetch an update that only was announced (when
    /// on) and tell the UI whether quitting installs. Handled on the update thread because
    /// the change arrives while the settings are locked, and reading the flag locks them.
    AutoChanged(bool),
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

pub struct UpdateService {
    checker: Arc<dyn Checker>,
    auto: AutoFlag,
    emit: StateEmit,
    state: Mutex<UpdateState>,
    /// Set once an install ran, so quitting after "Restart to update" does not install twice.
    installed: Mutex<bool>,
    /// The found update was fetched because the user asked (Check now, Download), so it
    /// installs on quit even with "Install updates automatically" off.
    asked: Mutex<bool>,
    wake: Mutex<Option<Sender<Wake>>>,
}

impl UpdateService {
    /// The service and its thread, which checks on `options`' schedule and on request.
    pub fn spawn(
        checker: Arc<dyn Checker>,
        auto: AutoFlag,
        emit: StateEmit,
        options: UpdateOptions,
    ) -> Arc<UpdateService> {
        let service = Arc::new(UpdateService::new(checker, auto, emit));
        let (tx, rx) = mpsc::channel();
        *lock(&service.wake) = Some(tx);
        let worker = Arc::downgrade(&service);
        std::thread::Builder::new()
            .name("prism-update".into())
            .spawn(move || {
                let mut due = options.schedule.then(|| Instant::now() + options.first);
                loop {
                    let message = match due {
                        Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    let Some(service) = worker.upgrade() else {
                        return;
                    };
                    match message {
                        Ok(Wake::Check) => {
                            service.run_check(true);
                        }
                        Ok(Wake::AutoChanged(on)) => service.follow_auto(on),
                        Err(RecvTimeoutError::Timeout) => {
                            let wait = match service.run_check(false) {
                                Outcome::Done => options.every,
                                Outcome::RetrySoon => options.retry,
                            };
                            due = Some(Instant::now() + wait);
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .expect("spawning the update thread");
        service
    }

    fn new(checker: Arc<dyn Checker>, auto: AutoFlag, emit: StateEmit) -> UpdateService {
        UpdateService {
            checker,
            auto,
            emit,
            state: Mutex::new(UpdateState::default()),
            installed: Mutex::new(false),
            asked: Mutex::new(false),
            wake: Mutex::new(None),
        }
    }

    pub fn state(&self) -> UpdateState {
        lock(&self.state).clone()
    }

    /// "Check now": runs on the update thread, the result arrives as `update://state`.
    pub fn check_now(&self) {
        self.send(Wake::Check);
    }

    /// Follows "Install updates automatically".
    pub fn auto_changed(&self, on: bool) {
        self.send(Wake::AutoChanged(on));
    }

    fn follow_auto(&self, on: bool) {
        if on {
            self.download_announced();
        }
        self.set(|_| {});
    }

    /// Whether [`UpdateService::install_on_quit`] would install now.
    fn would_install_on_quit(&self, status: UpdateStatus) -> bool {
        status == UpdateStatus::Ready
            && !*lock(&self.installed)
            && (*lock(&self.asked) || (self.auto)())
    }

    fn send(&self, wake: Wake) {
        if let Some(tx) = lock(&self.wake).as_ref() {
            let _ = tx.send(wake);
        }
    }

    fn set(&self, change: impl FnOnce(&mut UpdateState)) {
        let status = {
            let mut state = lock(&self.state);
            change(&mut state);
            state.status
        };
        // Outside the state lock: the auto flag reads the settings.
        let installs_on_quit = self.would_install_on_quit(status);
        let snapshot = {
            let mut state = lock(&self.state);
            state.installs_on_quit = installs_on_quit;
            state.clone()
        };
        (self.emit)(&snapshot);
    }

    fn run_check(&self, by_user: bool) -> Outcome {
        let status = self.state().status;
        if matches!(status, UpdateStatus::Ready | UpdateStatus::Downloading) {
            return Outcome::Done;
        }
        if !by_user && !self.checker.online() {
            return Outcome::RetrySoon;
        }
        self.set(|state| {
            state.status = UpdateStatus::Checking;
            state.progress = None;
        });
        match self.checker.check() {
            Err(err) => {
                eprintln!("checking for updates failed: {err}");
                self.set(|state| {
                    state.status = UpdateStatus::Failed;
                    state.last_check = Some(now_ms());
                });
            }
            Ok(None) => self.set(|state| {
                *state = UpdateState {
                    status: UpdateStatus::UpToDate,
                    last_check: Some(now_ms()),
                    ..UpdateState::default()
                };
            }),
            Ok(Some(found)) => {
                let download = by_user || (self.auto)();
                self.set(|state| {
                    *state = UpdateState {
                        status: if download {
                            UpdateStatus::Downloading
                        } else {
                            UpdateStatus::Available
                        },
                        version: Some(found.version),
                        notes: found.notes,
                        progress: None,
                        last_check: Some(now_ms()),
                        installs_on_quit: false,
                    };
                });
                *lock(&self.asked) = by_user;
                if download {
                    self.download();
                }
            }
        }
        if self.state().status == UpdateStatus::Failed {
            Outcome::RetrySoon
        } else {
            Outcome::Done
        }
    }

    fn download_announced(&self) {
        if self.state().status != UpdateStatus::Available {
            return;
        }
        self.set(|state| state.status = UpdateStatus::Downloading);
        self.download();
    }

    fn download(&self) {
        let mut shown = -1.0_f32;
        let result = self.checker.download(&mut |share| {
            let share = share.clamp(0.0, 1.0);
            // One event per percent is plenty for the UI.
            if share - shown >= 0.01 {
                shown = share;
                self.set(|state| state.progress = Some(share));
            }
        });
        match result {
            Ok(()) => self.set(|state| {
                state.status = UpdateStatus::Ready;
                state.progress = None;
            }),
            Err(err) => {
                eprintln!("downloading the update failed: {err}");
                self.set(|state| {
                    state.status = UpdateStatus::Failed;
                    state.progress = None;
                });
            }
        }
    }

    /// Prism is quitting: a ready update is installed now, if "Install updates automatically"
    /// is on or the user fetched it. Returns whether one was.
    pub fn install_on_quit(&self) -> bool {
        if !self.would_install_on_quit(self.state().status) {
            return false;
        }
        self.install(false).is_ok()
    }

    /// "Restart to update": installs the ready update. The caller relaunches Prism (on
    /// Windows the installer does, and this call does not return).
    pub fn install_for_restart(&self) -> Result<(), String> {
        self.install(true)
    }

    fn install(&self, relaunch: bool) -> Result<(), String> {
        if self.state().status != UpdateStatus::Ready {
            return Err("no update is ready".into());
        }
        let mut installed = lock(&self.installed);
        if *installed {
            return Err("the update is already installed".into());
        }
        match self.checker.install(relaunch) {
            Ok(()) => {
                *installed = true;
                Ok(())
            }
            Err(err) => {
                eprintln!("installing the update failed: {err}");
                drop(installed);
                self.set(|state| state.status = UpdateStatus::Failed);
                Err(err)
            }
        }
    }
}

/// Whether a scheduled check is worth running. Only clear signs of no network count as
/// offline: no name resolution, or every address timing out or unreachable. Any other probe
/// error (a refused or reset connection, as behind a proxy that blocks direct routes) still
/// runs the check, which knows the system's proxy settings better than a bare socket.
fn probe_online(
    addrs: std::io::Result<Vec<std::net::SocketAddr>>,
    connect: impl Fn(&std::net::SocketAddr) -> std::io::Result<()>,
) -> bool {
    use std::io::ErrorKind;
    let Ok(addrs) = addrs else {
        return false;
    };
    let mut offline = true;
    for addr in &addrs {
        match connect(addr) {
            Ok(()) => return true,
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::TimedOut
                        | ErrorKind::NetworkDown
                        | ErrorKind::NetworkUnreachable
                        | ErrorKind::HostUnreachable
                ) => {}
            Err(_) => offline = false,
        }
    }
    !offline
}

/// The real checker: `tauri-plugin-updater` with the endpoint and the public key from
/// `tauri.conf.json`.
pub struct PluginChecker {
    app: tauri::AppHandle,
    found: Mutex<Option<tauri_plugin_updater::Update>>,
    bytes: Mutex<Option<Vec<u8>>>,
}

impl PluginChecker {
    pub fn new(app: tauri::AppHandle) -> PluginChecker {
        PluginChecker {
            app,
            found: Mutex::new(None),
            bytes: Mutex::new(None),
        }
    }
}

impl Checker for PluginChecker {
    fn online(&self) -> bool {
        use std::net::{TcpStream, ToSocketAddrs};
        probe_online(
            ("github.com", 443)
                .to_socket_addrs()
                .map(|addrs| addrs.collect()),
            |addr| TcpStream::connect_timeout(addr, Duration::from_secs(5)).map(drop),
        )
    }

    fn check(&self) -> Result<Option<Found>, String> {
        use tauri_plugin_updater::UpdaterExt;
        let updater = self.app.updater().map_err(|err| err.to_string())?;
        let update =
            tauri::async_runtime::block_on(updater.check()).map_err(|err| err.to_string())?;
        let found = update.as_ref().map(|update| Found {
            version: update.version.clone(),
            notes: update.body.clone().filter(|body| !body.trim().is_empty()),
        });
        *lock(&self.found) = update;
        *lock(&self.bytes) = None;
        Ok(found)
    }

    fn download(&self, progress: &mut dyn FnMut(f32)) -> Result<(), String> {
        let update = lock(&self.found).clone().ok_or("no update was found")?;
        let mut received = 0_u64;
        let bytes = tauri::async_runtime::block_on(update.download(
            |chunk, total| {
                received += chunk as u64;
                if let Some(total) = total.filter(|total| *total > 0) {
                    progress(received as f32 / total as f32);
                }
            },
            || {},
        ))
        .map_err(|err| err.to_string())?;
        *lock(&self.bytes) = Some(bytes);
        Ok(())
    }

    fn install(&self, relaunch: bool) -> Result<(), String> {
        let update = lock(&self.found).clone().ok_or("no update was found")?;
        let bytes = lock(&self.bytes)
            .take()
            .ok_or("the update is not downloaded")?;
        update
            .restart_after_install(relaunch)
            .install(bytes)
            .map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeChecker {
        offline: AtomicBool,
        checks: Mutex<VecDeque<Result<Option<Found>, String>>>,
        downloads: Mutex<VecDeque<Result<(), String>>>,
        check_calls: AtomicUsize,
        download_calls: AtomicUsize,
        installs: Mutex<Vec<bool>>,
    }

    impl FakeChecker {
        fn answering(checks: Vec<Result<Option<Found>, String>>) -> Arc<FakeChecker> {
            let fake = FakeChecker::default();
            *lock(&fake.checks) = checks.into();
            Arc::new(fake)
        }
        fn downloads(self: &Arc<Self>, results: Vec<Result<(), String>>) -> Arc<Self> {
            *lock(&self.downloads) = results.into();
            Arc::clone(self)
        }
    }

    impl Checker for FakeChecker {
        fn online(&self) -> bool {
            !self.offline.load(Ordering::SeqCst)
        }
        fn check(&self) -> Result<Option<Found>, String> {
            self.check_calls.fetch_add(1, Ordering::SeqCst);
            lock(&self.checks).pop_front().unwrap_or(Ok(None))
        }
        fn download(&self, progress: &mut dyn FnMut(f32)) -> Result<(), String> {
            self.download_calls.fetch_add(1, Ordering::SeqCst);
            for share in [0.25, 0.5, 1.0] {
                progress(share);
            }
            lock(&self.downloads).pop_front().unwrap_or(Ok(()))
        }
        fn install(&self, relaunch: bool) -> Result<(), String> {
            lock(&self.installs).push(relaunch);
            Ok(())
        }
    }

    fn found(version: &str) -> Found {
        Found {
            version: version.into(),
            notes: Some("Fixes for Testhausen".into()),
        }
    }

    struct Harness {
        service: UpdateService,
        seen: Arc<Mutex<Vec<UpdateStatus>>>,
    }

    fn harness(checker: Arc<FakeChecker>, auto: bool) -> Harness {
        harness_with(checker, Arc::new(AtomicBool::new(auto)))
    }

    fn harness_with(checker: Arc<FakeChecker>, auto: Arc<AtomicBool>) -> Harness {
        let seen: Arc<Mutex<Vec<UpdateStatus>>> = Arc::default();
        let record = Arc::clone(&seen);
        let emit: StateEmit = Arc::new(move |state: &UpdateState| {
            let mut seen = lock(&record);
            if seen.last() != Some(&state.status) {
                seen.push(state.status);
            }
        });
        Harness {
            service: UpdateService::new(
                checker,
                Arc::new(move || auto.load(Ordering::SeqCst)),
                emit,
            ),
            seen,
        }
    }

    fn seen(harness: &Harness) -> Vec<UpdateStatus> {
        lock(&harness.seen).clone()
    }

    use UpdateStatus::*;

    #[test]
    fn found_update_downloads_and_becomes_ready() {
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        let h = harness(Arc::clone(&checker), true);
        h.service.run_check(false);
        assert_eq!(seen(&h), vec![Checking, Downloading, Ready]);
        let state = h.service.state();
        assert_eq!(state.version.as_deref(), Some("1.0.1"));
        assert_eq!(state.notes.as_deref(), Some("Fixes for Testhausen"));
        assert_eq!(state.progress, None);
        assert!(state.last_check.is_some());
        assert!(
            lock(&checker.installs).is_empty(),
            "never installs by itself"
        );
    }

    #[test]
    fn progress_is_reported_while_downloading() {
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        let progress: Arc<Mutex<Vec<f32>>> = Arc::default();
        let record = Arc::clone(&progress);
        let service = UpdateService::new(
            checker,
            Arc::new(|| true),
            Arc::new(move |state: &UpdateState| {
                if let Some(share) = state.progress {
                    lock(&record).push(share);
                }
            }),
        );
        service.run_check(false);
        assert_eq!(*lock(&progress), vec![0.25, 0.5, 1.0]);
    }

    #[test]
    fn a_failed_download_is_failed_and_the_next_check_retries() {
        let checker =
            FakeChecker::answering(vec![Ok(Some(found("1.0.1"))), Ok(Some(found("1.0.1")))])
                .downloads(vec![Err("signature mismatch".into()), Ok(())]);
        let h = harness(Arc::clone(&checker), true);
        h.service.run_check(false);
        assert_eq!(h.service.state().status, Failed);
        assert!(lock(&checker.installs).is_empty());
        assert!(
            !h.service.install_on_quit(),
            "nothing failed gets installed"
        );
        h.service.run_check(false);
        assert_eq!(h.service.state().status, Ready);
        assert_eq!(checker.check_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_failed_check_is_failed_and_retried() {
        let checker =
            FakeChecker::answering(vec![Err("unreachable".into()), Ok(Some(found("1.0.1")))]);
        let h = harness(Arc::clone(&checker), true);
        h.service.run_check(false);
        assert_eq!(h.service.state().status, Failed);
        assert!(h.service.state().last_check.is_some());
        h.service.run_check(false);
        assert_eq!(h.service.state().status, Ready);
    }

    #[test]
    fn offline_skips_the_scheduled_check() {
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        checker.offline.store(true, Ordering::SeqCst);
        let h = harness(Arc::clone(&checker), true);
        h.service.run_check(false);
        assert_eq!(checker.check_calls.load(Ordering::SeqCst), 0);
        assert!(seen(&h).is_empty());
        assert_eq!(h.service.state(), UpdateState::default());
    }

    #[test]
    fn nothing_new_is_up_to_date() {
        let checker = FakeChecker::answering(vec![Ok(None)]);
        let h = harness(Arc::clone(&checker), true);
        h.service.run_check(false);
        assert_eq!(seen(&h), vec![Checking, UpToDate]);
        assert_eq!(checker.download_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn auto_update_off_only_announces() {
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        let h = harness(Arc::clone(&checker), false);
        h.service.run_check(false);
        assert_eq!(seen(&h), vec![Checking, Available]);
        assert_eq!(checker.download_calls.load(Ordering::SeqCst), 0);
        assert!(!h.service.install_on_quit());

        // Turning it on (or asking) fetches the announced update.
        h.service.download_announced();
        assert_eq!(h.service.state().status, Ready);
        assert_eq!(checker.download_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn check_now_downloads_even_with_auto_update_off() {
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        checker.offline.store(true, Ordering::SeqCst);
        let h = harness(Arc::clone(&checker), false);
        h.service.run_check(true);
        assert_eq!(h.service.state().status, Ready);
    }

    #[test]
    fn ready_and_quit_installs_once_without_relaunch() {
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        let h = harness(Arc::clone(&checker), true);
        h.service.run_check(false);
        // Later checks leave a ready update alone; nothing installs while Prism runs.
        h.service.run_check(false);
        assert_eq!(checker.check_calls.load(Ordering::SeqCst), 1);
        assert!(lock(&checker.installs).is_empty());

        assert!(h.service.install_on_quit());
        assert_eq!(*lock(&checker.installs), vec![false]);
        assert!(!h.service.install_on_quit());
        assert_eq!(lock(&checker.installs).len(), 1);
    }

    #[test]
    fn restart_installs_with_relaunch_and_quit_does_not_repeat_it() {
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        let h = harness(Arc::clone(&checker), true);
        assert!(
            h.service.install_for_restart().is_err(),
            "nothing ready yet"
        );
        h.service.run_check(false);
        h.service.install_for_restart().unwrap();
        assert!(!h.service.install_on_quit());
        assert_eq!(*lock(&checker.installs), vec![true]);
    }

    #[test]
    fn state_is_camel_case_over_ipc() {
        let state = UpdateState {
            status: UpToDate,
            version: Some("1.0.1".into()),
            notes: None,
            progress: Some(0.5),
            last_check: Some(1),
            installs_on_quit: false,
        };
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::json!({
                "status": "upToDate",
                "version": "1.0.1",
                "notes": null,
                "progress": 0.5,
                "lastCheck": 1,
                "installsOnQuit": false
            })
        );
    }

    #[test]
    fn installs_on_quit_says_what_quitting_does() {
        let auto = Arc::new(AtomicBool::new(true));
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        let h = harness_with(Arc::clone(&checker), Arc::clone(&auto));
        assert!(!h.service.state().installs_on_quit);
        h.service.run_check(false);
        assert!(
            h.service.state().installs_on_quit,
            "downloaded with auto on"
        );

        auto.store(false, Ordering::SeqCst);
        h.service.follow_auto(false);
        assert!(!h.service.state().installs_on_quit);
        assert!(!h.service.install_on_quit());

        auto.store(true, Ordering::SeqCst);
        h.service.follow_auto(true);
        assert!(h.service.state().installs_on_quit);
        assert!(h.service.install_on_quit());
    }

    #[test]
    fn installs_on_quit_is_true_for_what_the_user_fetched() {
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1"))), Ok(None)]);
        let h = harness(Arc::clone(&checker), false);
        h.service.run_check(false);
        assert_eq!(h.service.state().status, Available);
        assert!(!h.service.state().installs_on_quit);
        // Download (Check now) on the announced update.
        let checker2 = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        let h2 = harness(Arc::clone(&checker2), false);
        h2.service.run_check(true);
        assert!(h2.service.state().installs_on_quit);
        assert!(h2.service.install_on_quit());
    }

    /// The setting change reaches the service while the settings are locked; the new state
    /// goes out from the update thread.
    #[test]
    fn changing_auto_emits_a_new_state() {
        let auto = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&auto);
        let seen: Arc<Mutex<Vec<bool>>> = Arc::default();
        let record = Arc::clone(&seen);
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        let service = UpdateService::spawn(
            Arc::clone(&checker) as Arc<dyn Checker>,
            Arc::new(move || flag.load(Ordering::SeqCst)),
            Arc::new(move |state: &UpdateState| lock(&record).push(state.installs_on_quit)),
            UpdateOptions {
                schedule: false,
                ..UpdateOptions::default()
            },
        );
        service.check_now();
        wait_for(&service, Ready);
        assert_eq!(lock(&seen).last(), Some(&true));
        // Check now counts as asked; an automatic download does not.
        *lock(&service.asked) = false;
        auto.store(false, Ordering::SeqCst);
        service.auto_changed(false);
        let deadline = Instant::now() + Duration::from_secs(5);
        while lock(&seen).last() != Some(&false) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!service.state().installs_on_quit);
    }

    #[test]
    fn schedule_is_a_minute_then_six_hours() {
        let options = UpdateOptions::default();
        assert!(options.schedule);
        assert_eq!(options.first, Duration::from_secs(60));
        assert_eq!(options.every, Duration::from_secs(21_600));
        assert_eq!(options.retry, Duration::from_secs(900));
    }

    /// Scheduled checks with a long regular interval and a short retry, so only the retry
    /// can bring a second check within the test.
    fn spawn_scheduled(checker: &Arc<FakeChecker>) -> Arc<UpdateService> {
        UpdateService::spawn(
            Arc::clone(checker) as Arc<dyn Checker>,
            Arc::new(|| true),
            Arc::new(|_: &UpdateState| {}),
            UpdateOptions {
                schedule: true,
                first: Duration::from_millis(10),
                every: Duration::from_secs(3600),
                retry: Duration::from_millis(20),
            },
        )
    }

    #[test]
    fn a_failed_scheduled_check_retries_soon() {
        let checker =
            FakeChecker::answering(vec![Err("unreachable".into()), Ok(Some(found("1.0.1")))]);
        let service = spawn_scheduled(&checker);
        wait_for(&service, Ready);
        assert_eq!(checker.check_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_failed_scheduled_download_retries_soon() {
        let checker =
            FakeChecker::answering(vec![Ok(Some(found("1.0.1"))), Ok(Some(found("1.0.1")))])
                .downloads(vec![Err("signature mismatch".into()), Ok(())]);
        let service = spawn_scheduled(&checker);
        wait_for(&service, Ready);
        assert_eq!(checker.download_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_skipped_scheduled_check_retries_soon() {
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        checker.offline.store(true, Ordering::SeqCst);
        let service = spawn_scheduled(&checker);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(checker.check_calls.load(Ordering::SeqCst), 0);
        checker.offline.store(false, Ordering::SeqCst);
        wait_for(&service, Ready);
    }

    #[test]
    fn a_successful_scheduled_check_waits_the_full_interval() {
        let checker = FakeChecker::answering(vec![Ok(None), Ok(None)]);
        let service = spawn_scheduled(&checker);
        wait_for(&service, UpToDate);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(checker.check_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn outcome_says_when_to_retry() {
        let checker = FakeChecker::answering(vec![Err("unreachable".into()), Ok(None)]);
        let h = harness(Arc::clone(&checker), true);
        assert_eq!(h.service.run_check(false), Outcome::RetrySoon);
        assert_eq!(h.service.run_check(false), Outcome::Done);
        checker.offline.store(true, Ordering::SeqCst);
        assert_eq!(h.service.run_check(false), Outcome::RetrySoon);
    }

    #[test]
    fn notify_only_skips_the_install_on_quit_unless_the_user_fetched_it() {
        // Downloaded automatically, then the user turned automatic updates off.
        let auto = Arc::new(AtomicBool::new(true));
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        let h = harness_with(Arc::clone(&checker), Arc::clone(&auto));
        h.service.run_check(false);
        assert_eq!(h.service.state().status, Ready);
        auto.store(false, Ordering::SeqCst);
        assert!(!h.service.install_on_quit());
        assert!(lock(&checker.installs).is_empty());
        // "Restart to update" still works.
        h.service.install_for_restart().unwrap();
        assert_eq!(*lock(&checker.installs), vec![true]);
    }

    #[test]
    fn notify_only_installs_on_quit_what_the_user_fetched() {
        let checker = FakeChecker::answering(vec![Ok(Some(found("1.0.1")))]);
        let h = harness(Arc::clone(&checker), false);
        h.service.run_check(true);
        assert_eq!(h.service.state().status, Ready);
        assert!(h.service.install_on_quit());
        assert_eq!(*lock(&checker.installs), vec![false]);
    }

    fn addr() -> std::net::SocketAddr {
        "127.0.0.1:443".parse().unwrap()
    }

    fn failing(kind: std::io::ErrorKind) -> impl Fn(&std::net::SocketAddr) -> std::io::Result<()> {
        move |_| Err(std::io::Error::from(kind))
    }

    #[test]
    fn probe_skips_only_on_clear_signs_of_no_network() {
        use std::io::ErrorKind;
        assert!(probe_online(Ok(vec![addr()]), |_| Ok(())));
        assert!(!probe_online(
            Ok(vec![addr()]),
            failing(ErrorKind::TimedOut)
        ));
        assert!(!probe_online(
            Ok(vec![addr()]),
            failing(ErrorKind::NetworkUnreachable)
        ));
        assert!(!probe_online(
            Err(std::io::Error::other("no such host")),
            |_| Ok(())
        ));
        assert!(!probe_online(Ok(vec![]), |_| Ok(())));
    }

    #[test]
    fn probe_errors_that_are_not_offline_still_check() {
        use std::io::ErrorKind;
        for kind in [
            ErrorKind::ConnectionRefused,
            ErrorKind::ConnectionReset,
            ErrorKind::PermissionDenied,
        ] {
            assert!(probe_online(Ok(vec![addr()]), failing(kind)), "{kind:?}");
        }
    }

    fn wait_for(service: &UpdateService, status: UpdateStatus) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while service.state().status != status {
            assert!(Instant::now() < deadline, "{:?}", service.state());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn without_a_schedule_only_check_now_checks() {
        let checker = FakeChecker::answering(vec![Ok(None)]);
        let service = UpdateService::spawn(
            Arc::clone(&checker) as Arc<dyn Checker>,
            Arc::new(|| true),
            Arc::new(|_: &UpdateState| {}),
            UpdateOptions {
                schedule: false,
                first: Duration::from_millis(1),
                every: Duration::from_millis(1),
                retry: Duration::from_millis(1),
            },
        );
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(checker.check_calls.load(Ordering::SeqCst), 0);
        service.check_now();
        wait_for(&service, UpToDate);
    }

    fn config() -> serde_json::Value {
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap()
    }

    /// Updates come from this repo's `latest.json` and nowhere else.
    #[test]
    fn update_endpoint_is_the_repo() {
        let updater = &config()["plugins"]["updater"];
        assert_eq!(updater["endpoints"], serde_json::json!([ENDPOINT]));
        assert_eq!(updater["windows"]["installMode"], "passive");
        // The signature must carry the version, so a relabelled latest.json cannot pair a
        // newer number with an older signed package. Every release since 1.0.0 is signed so.
        assert_eq!(updater["requireSignedVersion"], true);
        assert!(updater.get("dangerousInsecureTransportProtocol").is_none());
        assert!(updater.get("dangerousAcceptInvalidCerts").is_none());
    }

    /// The public key of the owner's `prism-updater` pair (minisign key id 1BA5D225E0B049BE).
    #[test]
    fn pubkey_is_pinned() {
        const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDFCQTVEMjI1RTBCMDQ5QkUKUldTK1NiRGdKZEtsRzdrcFIySmlSa0hCZ3J2LzY5YXNFU0U4OHRZZG12UnBPR0FJRDhFMVdydjEK";
        assert_eq!(config()["plugins"]["updater"]["pubkey"], PUBKEY);
    }

    /// Only the release workflow passes the overlay that signs updater artifacts; a source
    /// build has no key and must not try.
    #[test]
    fn updater_artifacts_only_come_from_the_release_overlay() {
        assert!(config()["bundle"].get("createUpdaterArtifacts").is_none());
        let overlay: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.updater.conf.json")).unwrap();
        assert_eq!(overlay["bundle"]["createUpdaterArtifacts"], true);
    }
}
