//! The service against the fake server: `MemoryStore` credentials, a temp settings file, a
//! recording emit and short timings.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

use rusqlite::OptionalExtension;
use tempfile::TempDir;

use super::keychain::MemoryStore;
use super::testserver::{FakeLibrary, FakeServer, Fault, FAKE_PASSWORD, FAKE_USER};
use super::*;

const WAIT: Duration = Duration::from_secs(15);

fn options() -> ServiceOptions {
    ServiceOptions {
        first_delay: Duration::from_millis(20),
        interval: Duration::from_secs(3600),
        offline_retry: Duration::from_millis(100),
        ..ServiceOptions::default()
    }
}

/// A credential store that counts reads.
#[derive(Default)]
struct CountingStore {
    inner: MemoryStore,
    reads: AtomicUsize,
}

impl CredentialStore for CountingStore {
    fn get(&self, url: &str, user: &str) -> anyhow::Result<Option<Secret>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.get(url, user)
    }

    fn set(&self, url: &str, user: &str, password: &Secret) -> anyhow::Result<()> {
        self.inner.set(url, user, password)
    }

    fn delete(&self, url: &str, user: &str) -> anyhow::Result<()> {
        self.inner.delete(url, user)
    }
}

struct Env {
    _dir: TempDir,
    path: PathBuf,
    db: Arc<Db>,
    settings: Arc<SettingsStore>,
    creds: Arc<MemoryStore>,
    events: Arc<Mutex<Vec<NavidromeEvent>>>,
    service: Arc<NavidromeService>,
}

/// Starts the service; `prepare` may save an account first (settings path, keychain).
fn start(prepare: impl FnOnce(&Path, &MemoryStore)) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let creds = Arc::new(MemoryStore::new());
    prepare(&path, &creds);
    let db = Arc::new(Db::open_in_memory().unwrap());
    let settings = Arc::new(SettingsStore::load(&path));
    let events: Arc<Mutex<Vec<NavidromeEvent>>> = Arc::default();
    let recorded = Arc::clone(&events);
    let service = NavidromeService::spawn_with_options(
        Arc::clone(&db),
        Arc::clone(&settings),
        Arc::clone(&creds) as Arc<dyn CredentialStore>,
        Arc::new(move |event| guard(&recorded).push(event)),
        options(),
    );
    Env {
        _dir: dir,
        path,
        db,
        settings,
        creds,
        events,
        service,
    }
}

/// Saves `server`'s fake account as connected (settings and keychain).
fn saved_account(server: &FakeServer, with_password: bool) -> impl FnOnce(&Path, &MemoryStore) {
    let url = server.url();
    move |path, creds| {
        SettingsStore::load(path)
            .set_navidrome_account(Some((url.clone(), FAKE_USER.into())))
            .unwrap();
        if with_password {
            creds
                .set(&url, FAKE_USER, &Secret::new(FAKE_PASSWORD))
                .unwrap();
        }
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Env {
    fn connect(&self, server: &FakeServer) -> NavidromeStatus {
        self.service
            .connect(&server.url(), FAKE_USER, Secret::new(FAKE_PASSWORD))
            .unwrap()
    }

    fn wait_state(&self, state: LinkState) {
        wait_for(&format!("{state:?}"), || {
            self.service.status().state == state
        });
    }

    fn wait_idle(&self) {
        assert!(self.service.wait_idle(WAIT), "the service did not settle");
    }

    fn count(&self, sql: &str) -> i64 {
        let conn = self.db.0.lock().unwrap();
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn nd_tracks(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM tracks WHERE source = 'navidrome'")
    }

    fn nd_albums(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM albums WHERE source = 'navidrome'")
    }

    fn state(&self, key: &str) -> Option<String> {
        let conn = self.db.0.lock().unwrap();
        conn.query_row(
            "SELECT value FROM navidrome_state WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
    }

    fn events(&self) -> Vec<NavidromeEvent> {
        guard(&self.events).clone()
    }

    fn settings_events(&self) -> Vec<Settings> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                NavidromeEvent::SettingsChanged(settings) => Some(settings),
                _ => None,
            })
            .collect()
    }

    fn settings_file(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }
}

fn requests(server: &FakeServer, method: &str) -> usize {
    server
        .requests()
        .iter()
        .filter(|r| r.method == method)
        .count()
}

#[test]
fn connect_stores_password_in_keychain_only() {
    let server = FakeServer::start(FakeLibrary::synthetic(2, 3));
    let env = start(|_, _| {});
    let status = env.connect(&server);
    assert!(status.configured);
    assert_eq!(status.url.as_deref(), Some(server.url().as_str()));
    env.wait_state(LinkState::Online);
    env.wait_idle();
    assert_eq!(env.nd_tracks(), 6);

    let file = env.settings_file();
    assert!(file.contains(&server.url()), "{file}");
    assert!(file.contains("\"navidromeUser\": \"demo\""), "{file}");
    assert!(!file.contains(FAKE_PASSWORD), "{file}");
    assert!(!file.to_lowercase().contains("password"), "{file}");
    assert_eq!(
        env.creds
            .get(&server.url(), FAKE_USER)
            .unwrap()
            .map(|s| s.expose().to_owned())
            .as_deref(),
        Some(FAKE_PASSWORD)
    );
    // The UI's settings store hears about the account.
    let saved = env.settings_events();
    assert_eq!(saved.len(), 1);
    assert_eq!(
        saved[0].navidrome_url.as_deref(),
        Some(server.url().as_str())
    );
    assert_eq!(saved[0].navidrome_user.as_deref(), Some(FAKE_USER));
    assert!(env.service.online_flag().load(Ordering::SeqCst));
    assert!(env.service.client().is_some());
}

#[test]
fn connect_normalizes_the_url_and_trims_the_user() {
    let server = FakeServer::start(FakeLibrary::synthetic(1, 2));
    let env = start(|_, _| {});
    let typed = format!("  {}/rest/  ", server.url());
    let status = env
        .service
        .connect(&typed, "  demo \t", Secret::new(FAKE_PASSWORD))
        .unwrap();
    assert_eq!(status.url.as_deref(), Some(server.url().as_str()));
    assert_eq!(status.user.as_deref(), Some(FAKE_USER));
    let saved = env.settings.get();
    assert_eq!(saved.navidrome_url.as_deref(), Some(server.url().as_str()));
    assert_eq!(saved.navidrome_user.as_deref(), Some(FAKE_USER));
    assert!(env.creds.get(&server.url(), FAKE_USER).unwrap().is_some());
    env.wait_idle();

    // An empty user is refused before anything goes out.
    let before = server.requests().len();
    let err = env
        .service
        .connect(&server.url(), "   ", Secret::new(FAKE_PASSWORD))
        .unwrap_err();
    assert_eq!(err.code, "wrongCredentials");
    assert_eq!(server.requests().len(), before);
    assert_eq!(
        env.settings.get().navidrome_user.as_deref(),
        Some(FAKE_USER)
    );
}

#[test]
fn connect_with_wrong_password_saves_nothing() {
    let server = FakeServer::start(FakeLibrary::synthetic(2, 3));
    let env = start(|_, _| {});
    let err = env
        .service
        .connect(&server.url(), FAKE_USER, Secret::new("not-the-password"))
        .unwrap_err();
    assert_eq!(err.code, "wrongCredentials");
    assert_eq!(env.settings.get().navidrome_url, None);
    assert!(env.creds.get(&server.url(), FAKE_USER).unwrap().is_none());
    assert!(env.settings_events().is_empty());
    assert_eq!(env.state("server"), None);
    assert_eq!(env.service.status().state, LinkState::Off);
    assert!(!env.service.online_flag().load(Ordering::SeqCst));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(requests(&server, "getIndexes"), 0);
}

#[test]
fn startup_with_saved_account_syncs() {
    let server = FakeServer::start(FakeLibrary::synthetic(3, 4));
    let env = start(saved_account(&server, true));
    assert!(env.service.status().configured);
    env.wait_state(LinkState::Online);
    wait_for("the first sync", || env.nd_tracks() == 12);
    env.wait_idle();
    let status = env.service.status();
    assert_eq!(status.state, LinkState::Online);
    assert_eq!(status.track_count, 12);
    assert_eq!(status.album_count, 3);
    assert!(status.last_sync.is_some());
    assert!(status.error.is_none());
    assert!(env
        .events()
        .iter()
        .any(|e| matches!(e, NavidromeEvent::LibraryChanged)));
}

#[test]
fn server_down_goes_offline_and_keeps_index() {
    let server = FakeServer::start(FakeLibrary::synthetic(2, 4));
    let env = start(|_, _| {});
    env.connect(&server);
    env.wait_idle();
    assert_eq!(env.service.status().state, LinkState::Online);
    assert_eq!(env.nd_tracks(), 8);

    server.set_fault(Fault::Refuse);
    env.service.sync_now();
    env.wait_state(LinkState::Offline);
    let status = env.service.status();
    assert_eq!(status.error.as_ref().map(|e| e.code), Some("unreachable"));
    assert_eq!(status.track_count, 8);
    assert_eq!(env.nd_tracks(), 8);
    assert!(!env.service.online_flag().load(Ordering::SeqCst));

    // The offline ping notices the server is back and runs a cycle.
    let pings = requests(&server, "ping");
    server.set_fault(Fault::None);
    env.wait_state(LinkState::Online);
    assert!(requests(&server, "ping") > pings);
    assert!(env.service.online_flag().load(Ordering::SeqCst));
    assert_eq!(env.nd_tracks(), 8);
}

#[test]
fn credential_errors_are_not_retried() {
    let server = FakeServer::start(FakeLibrary::synthetic(1, 2));
    let env = start(|_, _| {});
    env.connect(&server);
    env.wait_idle();
    server.set_fault(Fault::WrongPassword);
    env.service.sync_now();
    env.wait_state(LinkState::Error);
    env.wait_idle();
    assert_eq!(
        env.service.status().error.map(|e| e.code),
        Some("wrongCredentials")
    );
    assert!(!env.service.online_flag().load(Ordering::SeqCst));
    let calls = server.requests().len();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server.requests().len(), calls);
    assert_eq!(env.nd_tracks(), 2);
}

#[test]
fn refused_pass_is_a_visible_error() {
    let server = FakeServer::start(FakeLibrary::synthetic(3, 4));
    let env = start(|_, _| {});
    env.connect(&server);
    env.wait_idle();
    assert_eq!(env.nd_tracks(), 12);
    // The albums are still listed, but no song is: the pass is refused.
    server.edit(|lib| lib.songs.clear());
    env.service.sync_now();
    env.wait_state(LinkState::Error);
    env.wait_idle();
    // The server answers with these credentials: Navidrome tracks stay playable.
    assert!(env.service.online_flag().load(Ordering::SeqCst));
    let status = env.service.status();
    let error = status.error.expect("a failure");
    assert_eq!(error.code, FAILURE_SYNC_REFUSED);
    assert!(error.detail.is_some());
    assert_eq!(status.track_count, 12);
    assert_eq!(env.nd_tracks(), 12);
}

#[test]
fn switching_server_wipes_old_rows() {
    let first = FakeServer::start(FakeLibrary::synthetic(2, 3));
    let second = FakeServer::start(FakeLibrary::synthetic(1, 2));
    second.edit(|lib| {
        for song in &mut lib.songs {
            song.title = format!("Other {}", song.title);
        }
    });
    let env = start(|_, _| {});
    env.connect(&first);
    env.wait_idle();
    assert_eq!(env.nd_tracks(), 6);
    {
        let conn = env.db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO favorites (kind, item_id, source, added_at)
             VALUES ('track', 'navidrome:tr-5', 'navidrome', 1)",
            [],
        )
        .unwrap();
    }

    env.connect(&second);
    env.wait_state(LinkState::Online);
    env.wait_idle();
    assert_eq!(env.nd_tracks(), 2);
    assert_eq!(
        env.count(
            "SELECT COUNT(*) FROM tracks WHERE source = 'navidrome' AND title NOT LIKE 'Other %'"
        ),
        0
    );
    assert_eq!(
        env.count("SELECT COUNT(*) FROM favorites WHERE source = 'navidrome'"),
        0
    );
    // The old account's password is forgotten, the new one is kept.
    assert!(env.creds.get(&first.url(), FAKE_USER).unwrap().is_none());
    assert!(env.creds.get(&second.url(), FAKE_USER).unwrap().is_some());
    assert_eq!(
        env.state("server"),
        Some(sync::server_key(&second.url(), FAKE_USER))
    );
}

#[test]
fn disconnect_wipes_and_forgets() {
    let server = FakeServer::start(FakeLibrary::synthetic(2, 3));
    let env = start(|_, _| {});
    env.connect(&server);
    env.wait_idle();
    assert_eq!(env.nd_tracks(), 6);

    env.service.disconnect().unwrap();
    assert_eq!(env.nd_tracks(), 0);
    assert_eq!(env.nd_albums(), 0);
    assert_eq!(env.count("SELECT COUNT(*) FROM navidrome_state"), 0);
    assert!(env.creds.get(&server.url(), FAKE_USER).unwrap().is_none());
    let saved = env.settings.get();
    assert_eq!(saved.navidrome_url, None);
    assert_eq!(saved.navidrome_user, None);
    let file = env.settings_file();
    assert!(!file.contains(&server.url()), "{file}");
    let status = env.service.status();
    assert_eq!(status.state, LinkState::Off);
    assert!(!status.configured);
    assert_eq!(status.track_count, 0);
    assert!(env.service.client().is_none());
    assert!(!env.service.online_flag().load(Ordering::SeqCst));
    // The settings event of the disconnect carries no account.
    let events = env.settings_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].navidrome_url, None);
    // No cycle runs any more.
    let calls = server.requests().len();
    env.service.sync_now();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(server.requests().len(), calls);
}

/// A first sync of 600 songs whose second page stalls; returns once page 1 is indexed.
fn stalled_first_sync(env: &Env, server: &FakeServer) {
    server.set_fault(Fault::StallAfter {
        method: "search3",
        skip: 1,
        wait: Duration::from_millis(1500),
    });
    env.connect(server);
    wait_for("page 1", || env.nd_tracks() == 500);
    wait_for("the page 2 request", || requests(server, "search3") == 2);
}

#[test]
fn disconnect_during_a_stalled_first_sync_leaves_nothing() {
    let server = FakeServer::start(FakeLibrary::synthetic(2, 300));
    let env = start(|_, _| {});
    stalled_first_sync(&env, &server);

    env.service.disconnect().unwrap();
    // The stalled page arrives after the wipe; nothing of it may land.
    env.wait_idle();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(env.nd_tracks(), 0);
    assert_eq!(env.nd_albums(), 0);
    assert_eq!(env.state("last_modified"), None);
    assert_eq!(env.count("SELECT COUNT(*) FROM navidrome_state"), 0);
    assert_eq!(env.service.status().state, LinkState::Off);
}

#[test]
fn switching_server_during_a_stalled_first_sync_leaves_no_old_rows() {
    let first = FakeServer::start(FakeLibrary::synthetic(2, 300));
    // The same `lastModified` as the first server: an old pass that stored its value would
    // make the second server's pass look unchanged, and the old rows would stay.
    let mut other = FakeLibrary::synthetic(1, 3);
    for song in &mut other.songs {
        song.title = format!("Other {}", song.title);
    }
    let second = FakeServer::start(other);
    let env = start(|_, _| {});
    stalled_first_sync(&env, &first);

    env.connect(&second);
    // The first server's pass ends with its stalled page, then the second one syncs.
    env.wait_idle();
    env.wait_state(LinkState::Online);
    assert_eq!(env.nd_tracks(), 3);
    assert_eq!(env.nd_albums(), 1);
    assert_eq!(
        env.count(
            "SELECT COUNT(*) FROM tracks WHERE source = 'navidrome' AND title NOT LIKE 'Other %'"
        ),
        0
    );
    assert!(env.state("last_modified").is_some());
    assert_eq!(
        env.state("server"),
        Some(sync::server_key(&second.url(), FAKE_USER))
    );
    assert_eq!(
        env.service.status().url.as_deref(),
        Some(second.url().as_str())
    );
}

#[test]
fn sync_now_during_a_cycle_does_not_start_a_second_one() {
    let server = FakeServer::start(FakeLibrary::synthetic(2, 3));
    server.set_fault(Fault::Stall {
        method: "getIndexes",
        after_bytes: 0,
        wait: Duration::from_millis(600),
    });
    let env = start(|_, _| {});
    env.connect(&server);
    wait_for("the first cycle", || requests(&server, "getIndexes") == 1);
    for _ in 0..5 {
        env.service.sync_now();
    }
    std::thread::sleep(Duration::from_millis(100));
    // Still the one cycle: the kicks wait for it and run once after it.
    assert_eq!(requests(&server, "getIndexes"), 1);
    env.wait_idle();
    assert_eq!(requests(&server, "getIndexes"), 2);
    assert_eq!(env.service.status().state, LinkState::Online);
}

#[test]
fn shutdown_returns_promptly_during_a_stalled_call() {
    let server = FakeServer::start(FakeLibrary::synthetic(1, 2));
    server.set_fault(Fault::Stall {
        method: "getIndexes",
        after_bytes: 0,
        wait: Duration::from_secs(3),
    });
    let env = start(|_, _| {});
    env.connect(&server);
    wait_for("the stalled call", || requests(&server, "getIndexes") == 1);
    let started = Instant::now();
    env.service.shutdown();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "shutdown took {:?}",
        started.elapsed()
    );
    // The cancelled pass writes nothing once its call returns.
    assert!(env.service.wait_idle(WAIT));
    assert_eq!(env.nd_tracks(), 0);
    assert_eq!(env.state("last_modified"), None);
}

#[test]
fn status_events_carry_no_password() {
    let server = FakeServer::start(FakeLibrary::synthetic(2, 3));
    let env = start(|_, _| {});
    let _ = env
        .service
        .connect(&server.url(), FAKE_USER, Secret::new("demo-pass-wrong"));
    env.connect(&server);
    env.wait_idle();
    server.set_fault(Fault::Refuse);
    env.service.sync_now();
    env.wait_state(LinkState::Offline);
    server.set_fault(Fault::None);
    env.wait_state(LinkState::Online);
    env.service.disconnect().unwrap();

    let events = env.events();
    assert!(events
        .iter()
        .any(|e| matches!(e, NavidromeEvent::Status(_))));
    for event in &events {
        let debug = format!("{event:?}");
        assert!(!debug.contains(FAKE_PASSWORD), "{debug}");
        let json = match event {
            NavidromeEvent::Status(status) => serde_json::to_string(status).unwrap(),
            NavidromeEvent::SettingsChanged(settings) => serde_json::to_string(settings).unwrap(),
            _ => String::new(),
        };
        assert!(!json.contains(FAKE_PASSWORD), "{json}");
    }
    let status = format!("{:?}", env.service.status());
    assert!(!status.contains(FAKE_PASSWORD));
}

#[test]
fn missing_keychain_entry_is_an_error_state() {
    let server = FakeServer::start(FakeLibrary::synthetic(1, 2));
    let env = start(saved_account(&server, false));
    let status = env.service.status();
    assert!(status.configured);
    assert_eq!(status.state, LinkState::Error);
    assert_eq!(
        status.error.as_ref().map(|e| e.code),
        Some("wrongCredentials")
    );
    assert!(env.service.client().is_none());
    assert!(!env.service.online_flag().load(Ordering::SeqCst));
    env.service.sync_now();
    std::thread::sleep(Duration::from_millis(150));
    assert!(server.requests().is_empty());
}

#[test]
fn incomplete_account_never_reads_the_keychain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, br#"{ "navidromeUrl": "http://127.0.0.1:1" }"#).unwrap();
    let creds = Arc::new(CountingStore::default());
    let service = NavidromeService::spawn_with_options(
        Arc::new(Db::open_in_memory().unwrap()),
        Arc::new(SettingsStore::load(&path)),
        Arc::clone(&creds) as Arc<dyn CredentialStore>,
        Arc::new(|_| {}),
        options(),
    );
    let status = service.status();
    assert!(!status.configured);
    assert_eq!(status.state, LinkState::Off);
    service.sync_now();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(creds.reads.load(Ordering::SeqCst), 0);
    service.shutdown();
}

/// A keychain that matches entries ignoring case, as Windows Credential Manager does.
#[derive(Default)]
struct FoldingStore {
    entries: Mutex<std::collections::HashMap<String, Secret>>,
}

impl CredentialStore for FoldingStore {
    fn get(&self, url: &str, user: &str) -> anyhow::Result<Option<Secret>> {
        let key = account_key(url, user).to_lowercase();
        Ok(guard(&self.entries).get(&key).cloned())
    }

    fn set(&self, url: &str, user: &str, password: &Secret) -> anyhow::Result<()> {
        let key = account_key(url, user).to_lowercase();
        guard(&self.entries).insert(key, password.clone());
        Ok(())
    }

    fn delete(&self, url: &str, user: &str) -> anyhow::Result<()> {
        let key = account_key(url, user).to_lowercase();
        guard(&self.entries).remove(&key);
        Ok(())
    }
}

#[test]
fn reconnect_in_other_case_keeps_the_password_and_the_index() {
    let server = FakeServer::start(FakeLibrary::synthetic(2, 3));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let db = Arc::new(Db::open_in_memory().unwrap());
    let creds = Arc::new(FoldingStore::default());
    let events: Arc<Mutex<Vec<NavidromeEvent>>> = Arc::default();
    let spawn = |events: Arc<Mutex<Vec<NavidromeEvent>>>| {
        NavidromeService::spawn_with_options(
            Arc::clone(&db),
            Arc::new(SettingsStore::load(&path)),
            Arc::clone(&creds) as Arc<dyn CredentialStore>,
            Arc::new(move |event| guard(&events).push(event)),
            options(),
        )
    };
    let service = spawn(Arc::clone(&events));
    service
        .connect(&server.url(), "demo", Secret::new(FAKE_PASSWORD))
        .unwrap();
    assert!(service.wait_idle(WAIT));
    guard(&events).clear();

    // The same account, the user typed in another case.
    service
        .connect(&server.url(), "DEMO", Secret::new(FAKE_PASSWORD))
        .unwrap();
    assert!(service.wait_idle(WAIT));
    assert!(creds.get(&server.url(), "DEMO").unwrap().is_some());
    // No server change: nothing was wiped.
    assert!(!guard(&events)
        .iter()
        .any(|e| matches!(e, NavidromeEvent::FavoritesChanged)));
    {
        let conn = db.0.lock().unwrap();
        let tracks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tracks WHERE source = 'navidrome'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tracks, 6);
    }
    service.shutdown();

    // The next start finds the password.
    let restarted = spawn(Arc::default());
    let status = restarted.status();
    assert_eq!(status.user.as_deref(), Some("DEMO"));
    assert_ne!(status.state, LinkState::Error, "{:?}", status.error);
    restarted.shutdown();
}

#[test]
fn saved_account_starts_syncing() {
    let server = FakeServer::start(FakeLibrary::synthetic(1, 2));
    server.set_fault(Fault::Stall {
        method: "getIndexes",
        after_bytes: 0,
        wait: Duration::from_millis(300),
    });
    let env = start(saved_account(&server, true));
    assert_eq!(env.service.status().state, LinkState::Syncing);
    assert!(env.service.online_flag().load(Ordering::SeqCst));
    env.wait_state(LinkState::Online);
}

#[test]
fn a_panicking_cycle_does_not_stop_the_scheduler() {
    let server = FakeServer::start(FakeLibrary::synthetic(1, 2));
    let env = start(|_, _| {});
    env.service.core.panic_next.store(true, Ordering::SeqCst);
    env.connect(&server);
    // The panicking cycle ends; the scheduler is free for the next one.
    env.wait_idle();
    assert_eq!(env.nd_tracks(), 0);
    env.service.sync_now();
    env.wait_idle();
    env.wait_state(LinkState::Online);
    assert_eq!(env.nd_tracks(), 2);
}
