//! Navidrome (Subsonic API) as a source.
//!
//! [`NavidromeService`] owns the signed-in account, the status the UI shows and one scheduler
//! thread. The scheduler is the only place a sync cycle starts, and it never starts one while
//! another runs, so [`sync::sync_library`] has exactly one caller at a time. A cycle runs on a
//! worker thread of its own: the scheduler (and with it [`NavidromeService::shutdown`]) never
//! waits on the network.
//!
//! Connect and disconnect wipe the index of the old account. Each of them first bumps the
//! account epoch, then takes the database lock to wipe. A running cycle checks the epoch under
//! the same lock before every write, so once the wipe holds the lock nothing of the old pass
//! (rows, `lastModified`, status) is written after it. The connect and disconnect commands
//! never wait for the old cycle; the next account's first cycle does (see
//! [`NavidromeService::connect`]).

pub mod client;
pub mod covers;
pub mod keychain;
pub mod playlists;
pub mod scrobble;
pub mod stars;
pub mod stream;
pub mod sync;

#[cfg(test)]
pub(crate) mod testserver;

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::Serialize;

use crate::db::Db;
use crate::library::favorites::FavKind;
use crate::library::scan::lock as lock_db;
use crate::secret::Secret;
use crate::settings::{Settings, SettingsStore};
use client::{
    normalize_url, Account, NavidromeError, NavidromeFailure, ServerInfo, SubsonicClient,
};
use keychain::{account_key, CredentialStore};
use playlists::sync_playlists_cancellable;
use stars::FlushOutcome;
use sync::{
    ensure_server, server_key, sync_library_cancellable, wipe_navidrome, SyncError, SyncProgress,
    SyncReport,
};

pub const EVENT_STATUS: &str = "navidrome://status";

/// Failure codes of the service itself, next to [`NavidromeError::code`].
pub const FAILURE_SYNC_REFUSED: &str = "syncRefused";
pub const FAILURE_KEYCHAIN: &str = "keychain";
/// The keychain refused the account as too long (Windows caps `<user>@<url>` at 513 chars).
pub const FAILURE_KEYCHAIN_TOO_LONG: &str = "keychainTooLong";
pub const FAILURE_DATABASE: &str = "database";
pub const FAILURE_SETTINGS: &str = "settings";
pub const FAILURE_INTERNAL: &str = "internal";

/// What a heart on a Navidrome item answers while no account is signed in.
pub const NOT_CONNECTED: &str = "Navidrome is not connected";

/// Status events while a pass reports progress go out at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkState {
    /// No account.
    Off,
    Syncing,
    Online,
    /// The server cannot be reached; the index stays browsable, a ping retries.
    Offline,
    /// The server answered with something Prism cannot work with (credentials, TLS, not a
    /// Subsonic server, a refused pass). No automatic retry for credential errors.
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavidromeStatus {
    pub configured: bool,
    pub url: Option<String>,
    pub user: Option<String>,
    pub state: LinkState,
    pub last_sync: Option<i64>,
    pub track_count: u32,
    pub album_count: u32,
    pub progress: Option<SyncProgress>,
    pub error: Option<NavidromeFailure>,
}

impl NavidromeStatus {
    fn off() -> NavidromeStatus {
        NavidromeStatus {
            configured: false,
            url: None,
            user: None,
            state: LinkState::Off,
            last_sync: None,
            track_count: 0,
            album_count: 0,
            progress: None,
            error: None,
        }
    }
}

/// What the service tells the app shell.
#[derive(Clone, Debug)]
pub enum NavidromeEvent {
    Status(NavidromeStatus),
    LibraryChanged,
    FavoritesChanged,
    PlaylistsChanged,
    /// Connect and disconnect saved the account: the settings as saved, for
    /// `settings://changed`.
    SettingsChanged(Settings),
}

pub type NavidromeEmit = Arc<dyn Fn(NavidromeEvent) + Send + Sync>;

/// Deletes the cached cover files of the old server after a wipe.
pub type CoverPurge = Arc<dyn Fn() + Send + Sync>;

/// The cover purge, and whether a wipe happened before it was plugged in (at start).
struct PurgeSlot {
    hook: Option<CoverPurge>,
    pending: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ServiceOptions {
    /// The first cycle after start, when an account is saved.
    pub first_delay: Duration,
    /// Between two cycles while online.
    pub interval: Duration,
    /// Between two pings while offline.
    pub offline_retry: Duration,
    /// After a star flush found the server unreachable: the retries, then every cycle.
    pub star_retry: [Duration; 3],
}

impl Default for ServiceOptions {
    fn default() -> ServiceOptions {
        ServiceOptions {
            first_delay: Duration::from_secs(3),
            interval: Duration::from_secs(30 * 60),
            offline_retry: Duration::from_secs(60),
            star_retry: [
                Duration::from_secs(5),
                Duration::from_secs(30),
                Duration::from_secs(120),
            ],
        }
    }
}

/// The star flusher's schedule: a kick (a heart) or a retry due.
#[derive(Default)]
struct StarFlush {
    kick: bool,
    due: Option<Instant>,
    /// Retries used since the last kick.
    retry: usize,
    shutdown: bool,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn failure(code: &'static str, detail: impl Into<String>) -> NavidromeFailure {
    NavidromeFailure {
        code,
        detail: Some(detail.into()),
    }
}

/// A keychain error as the UI sees it; a value too long has a code of its own.
fn keychain_failure(err: &anyhow::Error) -> NavidromeFailure {
    let code = if keychain::is_too_long(err) {
        FAILURE_KEYCHAIN_TOO_LONG
    } else {
        FAILURE_KEYCHAIN
    };
    failure(code, format!("{err:#}"))
}

/// The account as typed: the URL normalized, the user trimmed and not empty.
fn checked_account(url: &str, user: &str, password: Secret) -> Result<Account, NavidromeError> {
    let url = normalize_url(url)?;
    let user = user.trim();
    if user.is_empty() {
        return Err(NavidromeError::WrongCredentials);
    }
    Ok(Account {
        url,
        user: user.to_owned(),
        password,
    })
}

fn new_client(account: Account) -> Result<SubsonicClient, NavidromeError> {
    SubsonicClient::new(account)
        .map_err(|_| NavidromeError::InvalidUrl("not a valid address".into()))
}

/// Navidrome rows of the index and the last sync time.
fn index_counts(conn: &Connection) -> rusqlite::Result<(u32, u32, Option<i64>)> {
    let count = |sql: &str| conn.query_row(sql, [], |row| row.get::<_, u32>(0));
    let last_sync = conn
        .query_row(
            "SELECT value FROM navidrome_state WHERE key = 'last_sync'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|value| value.parse::<i64>().ok());
    Ok((
        count("SELECT COUNT(*) FROM tracks WHERE source = 'navidrome'")?,
        count("SELECT COUNT(*) FROM albums WHERE source = 'navidrome'")?,
        last_sync,
    ))
}

/// Whether the server answers with these credentials: syncing, online, or an error the
/// server's answers do not cause (a refused pass, the local database). Off, offline and
/// credential, TLS, not-Subsonic or keychain errors are not.
fn answers(status: &NavidromeStatus) -> bool {
    match status.state {
        LinkState::Syncing | LinkState::Online => true,
        LinkState::Error => status
            .error
            .as_ref()
            .is_some_and(|e| e.code == FAILURE_SYNC_REFUSED || e.code == FAILURE_DATABASE),
        LinkState::Off | LinkState::Offline => false,
    }
}

/// Ends a cycle worker: clears `running` and wakes the scheduler, also when the cycle panics.
struct CycleDone<'a>(&'a Core);

impl Drop for CycleDone<'_> {
    fn drop(&mut self) {
        self.0.inner().running = false;
        self.0.wake.notify_all();
    }
}

/// Everything the scheduler, the cycle workers and the commands share.
struct Inner {
    status: NavidromeStatus,
    client: Option<Arc<SubsonicClient>>,
    /// Bumped by connect, disconnect and shutdown; a cycle of an older epoch is cancelled.
    epoch: u64,
    /// A cycle was asked for (connect, "Sync now").
    kick: bool,
    /// When the next scheduled cycle (or offline ping) is due.
    due: Option<Instant>,
    /// A cycle worker is running.
    running: bool,
    shutdown: bool,
}

struct Core {
    db: Arc<Db>,
    settings: Arc<SettingsStore>,
    creds: Arc<dyn CredentialStore>,
    emit: NavidromeEmit,
    options: ServiceOptions,
    inner: Mutex<Inner>,
    wake: Condvar,
    online: Arc<AtomicBool>,
    /// Serializes connect and disconnect.
    account: Mutex<()>,
    cover_purge: Mutex<PurgeSlot>,
    /// Serializes star flushes and the star import, so an import never works from a server
    /// listing taken before a flush that then cleared the intent.
    star_lock: Mutex<()>,
    star_flush: Mutex<StarFlush>,
    star_wake: Condvar,
    /// Test hook: the next cycle panics.
    #[cfg(test)]
    panic_next: AtomicBool,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl Core {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        guard(&self.inner)
    }

    fn counts(&self) -> (u32, u32, Option<i64>) {
        lock_db(&self.db)
            .ok()
            .and_then(|conn| index_counts(&conn).ok())
            .unwrap_or((0, 0, None))
    }

    /// Updates the online flag, releases the lock and emits the status.
    fn publish(&self, inner: MutexGuard<'_, Inner>) -> NavidromeStatus {
        let status = inner.status.clone();
        self.online.store(answers(&status), Ordering::SeqCst);
        drop(inner);
        (self.emit)(NavidromeEvent::Status(status.clone()));
        status
    }

    /// Applies `change` to the status when `epoch` is still current, and emits it.
    fn update(&self, epoch: u64, change: impl FnOnce(&mut NavidromeStatus)) {
        let mut inner = self.inner();
        if inner.epoch != epoch {
            return;
        }
        change(&mut inner.status);
        self.publish(inner);
    }

    /// Deletes the cached covers of the old server, right after a wipe committed. Runs under
    /// the account lock like the wipe, and outside `inner` and the database lock.
    fn purge_covers(&self) {
        let hook = {
            let mut slot = guard(&self.cover_purge);
            if slot.hook.is_none() {
                slot.pending = true;
            }
            slot.hook.clone()
        };
        if let Some(hook) = hook {
            hook();
        }
    }

    fn is_current(&self, epoch: u64) -> bool {
        self.inner().epoch == epoch
    }

    /// Sends the star queue. A dropped intent may have dropped its local favorite.
    fn flush_stars(&self, client: &SubsonicClient, epoch: u64) -> FlushOutcome {
        let _stars = guard(&self.star_lock);
        let outcome = stars::flush_cancellable(&self.db, client, &|| !self.is_current(epoch));
        if outcome.dropped > 0 && self.is_current(epoch) {
            (self.emit)(NavidromeEvent::FavoritesChanged);
        }
        outcome
    }

    /// Takes the server's stars for every item without a pending intent.
    fn import_stars(&self, client: &SubsonicClient, epoch: u64) -> Result<(), SyncError> {
        let _stars = guard(&self.star_lock);
        let starred = client.starred2()?;
        let changed = {
            let mut conn = lock_db(&self.db)?;
            if !self.is_current(epoch) {
                return Err(SyncError::Cancelled);
            }
            stars::import(&mut conn, &starred, now_ms())?
        };
        if changed && self.is_current(epoch) {
            (self.emit)(NavidromeEvent::FavoritesChanged);
        }
        Ok(())
    }

    /// The star flusher thread: flushes on a kick (a heart) and retries after an offline
    /// flush. It flushes only while the server answers; otherwise the next cycle (after the
    /// offline ping answers) sends the queue.
    fn flush_loop(self: Arc<Core>) {
        loop {
            {
                let mut state = guard(&self.star_flush);
                loop {
                    if state.shutdown {
                        return;
                    }
                    if state.kick {
                        state.kick = false;
                        state.retry = 0;
                        state.due = None;
                        break;
                    }
                    let now = Instant::now();
                    match state.due {
                        Some(at) if at <= now => {
                            state.due = None;
                            break;
                        }
                        Some(at) => {
                            state = self
                                .star_wake
                                .wait_timeout(state, at - now)
                                .unwrap_or_else(|e| e.into_inner())
                                .0;
                        }
                        None => {
                            state = self
                                .star_wake
                                .wait(state)
                                .unwrap_or_else(|e| e.into_inner());
                        }
                    }
                }
            }
            let target = {
                let inner = self.inner();
                inner
                    .client
                    .clone()
                    .filter(|_| answers(&inner.status))
                    .map(|client| (client, inner.epoch))
            };
            let offline = match target {
                Some((client, epoch)) => self.flush_stars(&client, epoch).offline,
                None => false,
            };
            let mut state = guard(&self.star_flush);
            if !offline {
                state.retry = 0;
            } else if !state.kick {
                state.due = self
                    .options
                    .star_retry
                    .get(state.retry)
                    .map(|delay| Instant::now() + *delay);
                state.retry += 1;
            }
        }
    }

    /// [`Core::is_current`] checked while holding the database lock, the way a wipe orders
    /// itself: a wipe that already holds the lock has bumped the epoch.
    fn is_current_under_db_lock(&self, epoch: u64) -> bool {
        match lock_db(&self.db) {
            Ok(_conn) => self.is_current(epoch),
            Err(_) => false,
        }
    }

    /// The scheduler thread: waits for a kick or the due time, then runs one cycle on a
    /// worker and waits for it before the next one.
    fn schedule(self: Arc<Core>) {
        loop {
            let (client, epoch, ping_first) = {
                let mut inner = self.inner();
                loop {
                    if inner.shutdown {
                        return;
                    }
                    if inner.running {
                        inner = self.wake.wait(inner).unwrap_or_else(|e| e.into_inner());
                        continue;
                    }
                    if inner.kick {
                        inner.kick = false;
                        break;
                    }
                    let now = Instant::now();
                    inner = match inner.due {
                        Some(at) if at <= now => break,
                        Some(at) => {
                            self.wake
                                .wait_timeout(inner, at - now)
                                .unwrap_or_else(|e| e.into_inner())
                                .0
                        }
                        None => self.wake.wait(inner).unwrap_or_else(|e| e.into_inner()),
                    };
                }
                inner.due = None;
                let Some(client) = inner.client.clone() else {
                    continue;
                };
                inner.running = true;
                let ping_first = inner.status.state == LinkState::Offline;
                (client, inner.epoch, ping_first)
            };
            let core = Arc::clone(&self);
            let spawned = std::thread::Builder::new()
                .name("prism-navidrome-cycle".into())
                .spawn(move || {
                    // Also on a panic: a stuck `running` would stop every later cycle.
                    let _done = CycleDone(&core);
                    core.run_cycle(&client, epoch, ping_first);
                });
            if let Err(err) = spawned {
                eprintln!("starting the Navidrome sync failed: {err}");
                self.inner().running = false;
            }
        }
    }

    /// One cycle; offline, a ping goes first and the cycle runs only when it answers.
    fn run_cycle(&self, client: &SubsonicClient, epoch: u64, ping_first: bool) {
        if ping_first {
            if let Err(err) = client.ping() {
                self.finish(epoch, Err(SyncError::Api(err)));
                return;
            }
        }
        self.update(epoch, |status| {
            status.state = LinkState::Syncing;
            status.progress = None;
            status.error = None;
        });
        let result = self.cycle(client, epoch);
        self.finish(epoch, result);
    }

    /// The steps of one cycle, in this order: flush stars, library, import stars, playlists,
    /// flush scrobbles. Later tasks add their steps here.
    fn cycle(&self, client: &SubsonicClient, epoch: u64) -> Result<SyncReport, SyncError> {
        #[cfg(test)]
        if self.panic_next.swap(false, Ordering::SeqCst) {
            panic!("test panic in the Navidrome cycle");
        }
        let flushed = self.flush_stars(client, epoch);
        if !self.is_current(epoch) {
            return Err(SyncError::Cancelled);
        }
        // A server that did not answer the flush would only cost the library step another
        // timeout.
        if flushed.offline {
            return Err(SyncError::Api(NavidromeError::Unreachable(
                "the server did not answer".into(),
            )));
        }
        let last_progress: Cell<Option<Instant>> = Cell::new(None);
        let progress = |progress: SyncProgress| {
            let finished = progress.total.is_some_and(|total| progress.done >= total);
            if !finished
                && last_progress
                    .get()
                    .is_some_and(|at| at.elapsed() < PROGRESS_EVERY)
            {
                return;
            }
            last_progress.set(Some(Instant::now()));
            self.update(epoch, |status| status.progress = Some(progress));
        };
        let changed = || {
            if self.is_current(epoch) {
                (self.emit)(NavidromeEvent::LibraryChanged);
            }
        };
        let cancelled = || !self.is_current(epoch);
        let report =
            sync_library_cancellable(&self.db, client, now_ms(), &progress, &changed, &cancelled)?;
        match self.import_stars(client, epoch) {
            // The server answered, but not with stars Prism can take: the stars wait for the
            // next cycle, the rest of this one goes on.
            Err(SyncError::Api(
                err @ (NavidromeError::Server { .. }
                | NavidromeError::NotAuthorized
                | NavidromeError::NotFound
                | NavidromeError::NotSubsonic(_)),
            )) => {
                eprintln!("Navidrome star import: {}", err.code());
            }
            result => result?,
        }
        if sync_playlists_cancellable(&self.db, client, now_ms(), &cancelled)?
            && self.is_current_under_db_lock(epoch)
        {
            (self.emit)(NavidromeEvent::PlaylistsChanged);
        }
        self.flush_scrobbles(client, epoch)?;
        Ok(report)
    }

    /// The status after a cycle, and when the next one is due.
    fn finish(&self, epoch: u64, result: Result<SyncReport, SyncError>) {
        if matches!(result, Err(SyncError::Cancelled)) {
            return;
        }
        let (tracks, albums, last_sync) = self.counts();
        let options = self.options;
        let mut inner = self.inner();
        if inner.epoch != epoch {
            return;
        }
        let now = Instant::now();
        let (state, error, due) = match result {
            Ok(_) => (LinkState::Online, None, Some(now + options.interval)),
            Err(SyncError::Api(err)) if err.is_offline() => (
                LinkState::Offline,
                Some(NavidromeFailure::from(&err)),
                Some(now + options.offline_retry),
            ),
            // Credentials, TLS, not a Subsonic server: retrying changes nothing until the
            // user acts ("Sync now", connect).
            Err(SyncError::Api(err)) => {
                (LinkState::Error, Some(NavidromeFailure::from(&err)), None)
            }
            Err(SyncError::Refused(reason)) => (
                LinkState::Error,
                Some(failure(FAILURE_SYNC_REFUSED, reason)),
                Some(now + options.interval),
            ),
            Err(SyncError::Db(err)) => (
                LinkState::Error,
                Some(failure(FAILURE_DATABASE, format!("{err:#}"))),
                Some(now + options.interval),
            ),
            Err(SyncError::Cancelled) => return,
        };
        if let Some(error) = &error {
            eprintln!("Navidrome sync: {}", error.code);
        }
        inner.due = due;
        let status = &mut inner.status;
        status.state = state;
        status.error = error;
        status.progress = None;
        status.track_count = tracks;
        status.album_count = albums;
        status.last_sync = last_sync;
        self.publish(inner);
    }
}

/// The Navidrome account, its status and the background sync.
pub struct NavidromeService {
    core: Arc<Core>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl NavidromeService {
    pub fn spawn(
        db: Arc<Db>,
        settings: Arc<SettingsStore>,
        creds: Arc<dyn CredentialStore>,
        emit: NavidromeEmit,
    ) -> Arc<NavidromeService> {
        NavidromeService::spawn_with_options(db, settings, creds, emit, ServiceOptions::default())
    }

    pub fn spawn_with_options(
        db: Arc<Db>,
        settings: Arc<SettingsStore>,
        creds: Arc<dyn CredentialStore>,
        emit: NavidromeEmit,
        options: ServiceOptions,
    ) -> Arc<NavidromeService> {
        let saved = settings.get();
        let mut status = NavidromeStatus::off();
        let mut client = None;
        let mut due = None;
        let mut purge_pending = false;
        // Only a complete account reads the keychain.
        if let (Some(url), Some(user)) = (saved.navidrome_url, saved.navidrome_user) {
            status.configured = true;
            status.url = Some(url.clone());
            status.user = Some(user.clone());
            match creds.get(&url, &user) {
                Ok(Some(password)) => match checked_account(&url, &user, password)
                    .and_then(new_client)
                {
                    Ok(built) => {
                        // A hand-edited account is another server: its index is not this one.
                        let key = server_key(&built.account().url, &built.account().user);
                        match ensure_server(&db, &key) {
                            Ok(wiped) => {
                                purge_pending = wiped;
                                client = Some(Arc::new(built));
                                status.state = LinkState::Syncing;
                                due = Some(Instant::now() + options.first_delay);
                            }
                            Err(err) => {
                                status.state = LinkState::Error;
                                status.error = Some(failure(FAILURE_DATABASE, format!("{err:#}")));
                            }
                        }
                    }
                    Err(err) => {
                        status.state = LinkState::Error;
                        status.error = Some(NavidromeFailure::from(&err));
                    }
                },
                Ok(None) => {
                    status.state = LinkState::Error;
                    status.error = Some(NavidromeFailure::from(&NavidromeError::WrongCredentials));
                }
                Err(err) => {
                    status.state = LinkState::Error;
                    status.error = Some(keychain_failure(&err));
                }
            }
        }
        if let Ok(conn) = lock_db(&db) {
            if let Ok((tracks, albums, last_sync)) = index_counts(&conn) {
                status.track_count = tracks;
                status.album_count = albums;
                status.last_sync = last_sync;
            }
        }
        let online = Arc::new(AtomicBool::new(answers(&status)));
        let core = Arc::new(Core {
            db,
            settings,
            creds,
            emit,
            options,
            inner: Mutex::new(Inner {
                status,
                client,
                epoch: 0,
                kick: false,
                due,
                running: false,
                shutdown: false,
            }),
            wake: Condvar::new(),
            online,
            account: Mutex::new(()),
            cover_purge: Mutex::new(PurgeSlot {
                hook: None,
                pending: purge_pending,
            }),
            star_lock: Mutex::new(()),
            star_flush: Mutex::new(StarFlush::default()),
            star_wake: Condvar::new(),
            #[cfg(test)]
            panic_next: AtomicBool::new(false),
        });
        let scheduler = Arc::clone(&core);
        let thread = std::thread::Builder::new()
            .name("prism-navidrome".into())
            .spawn(move || scheduler.schedule())
            .map_err(|err| eprintln!("starting the Navidrome scheduler failed: {err}"))
            .ok();
        // Not joined on shutdown: a flush may wait on the network, and it writes nothing
        // after the epoch changed.
        let flusher = Arc::clone(&core);
        if let Err(err) = std::thread::Builder::new()
            .name("prism-navidrome-stars".into())
            .spawn(move || flusher.flush_loop())
        {
            eprintln!("starting the Navidrome star flusher failed: {err}");
        }
        Arc::new(NavidromeService {
            core,
            thread: Mutex::new(thread),
        })
    }

    pub fn status(&self) -> NavidromeStatus {
        self.core.inner().status.clone()
    }

    /// The client of the signed-in account, `None` when there is none (or it failed).
    pub fn client(&self) -> Option<Arc<SubsonicClient>> {
        self.core.inner().client.clone()
    }

    /// True while the server is known to answer (`Syncing`, `Online`).
    pub fn online_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.core.online)
    }

    /// Pings the server with these credentials; saves nothing.
    pub fn test(
        &self,
        url: &str,
        user: &str,
        password: Secret,
    ) -> Result<ServerInfo, NavidromeError> {
        new_client(checked_account(url, user, password)?)?.ping()
    }

    /// Signs in: pings with the given password and only then stores the password in the
    /// keychain, saves url and user, wipes the index of another account and starts a cycle.
    ///
    /// The command itself never waits on a running cycle: the old account's cycle is
    /// cancelled, not joined. Its worker can still be in a network call, though, and the new
    /// account's first cycle starts only once that call returns (at most `API_TIMEOUT`).
    ///
    /// When the wipe (`ensure_server`) fails, the account is already switched (keychain and
    /// settings) but no client is kept: the status is `Error` with `database` until the next
    /// connect or app start.
    pub fn connect(
        &self,
        url: &str,
        user: &str,
        password: Secret,
    ) -> Result<NavidromeStatus, NavidromeFailure> {
        let core = &self.core;
        let _account = guard(&core.account);
        let client = checked_account(url, user, password)
            .and_then(new_client)
            .map_err(|e| NavidromeFailure::from(&e))?;
        client.ping().map_err(|e| NavidromeFailure::from(&e))?;
        let account = client.account();
        let (url, user) = (account.url.clone(), account.user.clone());

        let before = core.settings.get();
        let previous = before.navidrome_url.zip(before.navidrome_user);
        // The same keychain entry: Windows Credential Manager matches targets ignoring case,
        // so `Demo` after `demo` replaced the entry, and deleting the "old" one would delete
        // the new password.
        let new_key = account_key(&url, &user);
        let same = previous
            .as_ref()
            .is_some_and(|(u, n)| account_key(u, n).eq_ignore_ascii_case(&new_key));
        core.creds
            .set(&url, &user, &account.password)
            .map_err(|err| keychain_failure(&err))?;
        let saved = match core
            .settings
            .set_navidrome_account(Some((url.clone(), user.clone())))
        {
            Ok(saved) => saved,
            Err(err) => {
                if !same {
                    if let Err(err) = core.creds.delete(&url, &user) {
                        eprintln!("forgetting the new Navidrome password failed: {err:#}");
                    }
                }
                return Err(failure(FAILURE_SETTINGS, format!("{err:#}")));
            }
        };
        if !same {
            if let Some((old_url, old_user)) = &previous {
                if let Err(err) = core.creds.delete(old_url, old_user) {
                    eprintln!("forgetting the old Navidrome password failed: {err:#}");
                }
            }
        }
        (core.emit)(NavidromeEvent::SettingsChanged(saved));

        // Cancel the running cycle of the old account before the wipe.
        {
            let mut inner = core.inner();
            inner.epoch += 1;
            inner.client = None;
        }
        let wiped = ensure_server(&core.db, &server_key(&url, &user));
        if matches!(wiped, Ok(true)) {
            core.purge_covers();
            emit_wiped(core);
        }
        let (tracks, albums, last_sync) = core.counts();
        let mut inner = core.inner();
        let status = &mut inner.status;
        status.configured = true;
        status.url = Some(url);
        status.user = Some(user);
        status.track_count = tracks;
        status.album_count = albums;
        status.last_sync = last_sync;
        status.progress = None;
        if let Err(err) = wiped {
            let error = failure(FAILURE_DATABASE, format!("{err:#}"));
            status.state = LinkState::Error;
            status.error = Some(error.clone());
            core.publish(inner);
            return Err(error);
        }
        status.state = LinkState::Syncing;
        status.error = None;
        inner.client = Some(Arc::new(client));
        inner.kick = true;
        inner.due = None;
        core.wake.notify_all();
        Ok(core.publish(inner))
    }

    /// Signs out: deletes the keychain entry, clears url and user, wipes every Navidrome row.
    pub fn disconnect(&self) -> anyhow::Result<()> {
        let core = &self.core;
        let _account = guard(&core.account);
        // Cancel the running cycle before the wipe.
        {
            let mut inner = core.inner();
            inner.epoch += 1;
            inner.client = None;
            inner.kick = false;
            inner.due = None;
        }
        let mut first_error: Option<anyhow::Error> = None;
        let saved = core.settings.get();
        if let (Some(url), Some(user)) = (&saved.navidrome_url, &saved.navidrome_user) {
            if let Err(err) = core.creds.delete(url, user) {
                first_error.get_or_insert(err);
            }
        }
        match core.settings.set_navidrome_account(None) {
            Ok(saved) => (core.emit)(NavidromeEvent::SettingsChanged(saved)),
            Err(err) => {
                first_error.get_or_insert(err);
            }
        }
        let wiped = (|| -> anyhow::Result<()> {
            let mut conn = lock_db(&core.db)?;
            let tx = conn.transaction()?;
            wipe_navidrome(&tx)?;
            tx.execute("DELETE FROM navidrome_state", [])?;
            tx.commit()?;
            Ok(())
        })();
        match wiped {
            Ok(()) => {
                core.purge_covers();
                emit_wiped(core);
            }
            Err(err) => {
                first_error.get_or_insert(err);
            }
        }
        let (tracks, albums, _) = core.counts();
        let mut inner = core.inner();
        inner.status = NavidromeStatus {
            track_count: tracks,
            album_count: albums,
            ..NavidromeStatus::off()
        };
        core.publish(inner);
        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Plugs in the cover purge every wipe runs after its commit. A wipe at start (a
    /// hand-edited account) happened before this, so it purges now.
    pub fn set_cover_purge(&self, hook: CoverPurge) {
        let _account = guard(&self.core.account);
        let pending = {
            let mut slot = guard(&self.core.cover_purge);
            slot.hook = Some(Arc::clone(&hook));
            std::mem::take(&mut slot.pending)
        };
        if pending {
            hook();
        }
    }

    /// Runs a cycle now (or right after the running one).
    pub fn sync_now(&self) {
        let mut inner = self.core.inner();
        if inner.client.is_some() {
            inner.kick = true;
            self.core.wake.notify_all();
        }
    }

    /// Asks for the star queue to be flushed now. It is sent only while the server answers;
    /// otherwise it waits for the next cycle. An unreachable server retries after 5 s, 30 s
    /// and 2 min, then every cycle.
    pub fn kick_flush(&self) {
        let mut state = guard(&self.core.star_flush);
        state.kick = true;
        self.core.star_wake.notify_all();
    }

    /// A heart on a Navidrome item: the local favorite at once, the server through the star
    /// queue. Fails with [`NOT_CONNECTED`] while no account is signed in. Runs under the
    /// account lock, so the intent is queued for the account it was made for: a connect or
    /// disconnect wipes it, never sends it to another server.
    pub fn set_favorite(&self, kind: FavKind, id: &str, on: bool) -> anyhow::Result<bool> {
        let changed = {
            let _account = guard(&self.core.account);
            if self.core.inner().status.state == LinkState::Off {
                anyhow::bail!(NOT_CONNECTED);
            }
            let conn = lock_db(&self.core.db)?;
            stars::set_favorite(&conn, kind, id, on, now_ms())?
        };
        self.kick_flush();
        Ok(changed)
    }

    /// Stops the scheduler and cancels a running cycle. Returns promptly also while a cycle
    /// waits on the network: that worker writes nothing more and ends with its request.
    pub fn shutdown(&self) {
        {
            let mut stars = guard(&self.core.star_flush);
            stars.shutdown = true;
            self.core.star_wake.notify_all();
        }
        {
            let mut inner = self.core.inner();
            inner.shutdown = true;
            inner.epoch += 1;
            self.core.wake.notify_all();
        }
        if let Some(thread) = guard(&self.thread).take() {
            let _ = thread.join();
        }
    }

    /// Waits until no cycle runs and none is asked for.
    #[cfg(test)]
    fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let inner = self.core.inner();
                if !inner.running && !inner.kick {
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for NavidromeService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn emit_wiped(core: &Core) {
    (core.emit)(NavidromeEvent::LibraryChanged);
    (core.emit)(NavidromeEvent::FavoritesChanged);
    (core.emit)(NavidromeEvent::PlaylistsChanged);
}

#[cfg(test)]
mod tests;
