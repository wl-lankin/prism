//! Stars against the fake server: the queue and its flush, the import, and the service's
//! kick, retry and restart behavior.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::library::favorites::{self, FavoriteIds};
use crate::navidrome::client::Child;
use crate::navidrome::keychain::{CredentialStore, MemoryStore};
use crate::navidrome::sync::{ensure_server, server_key, wipe_navidrome};
use crate::navidrome::testserver::{FakeLibrary, FakeServer, Fault, FAKE_PASSWORD, FAKE_USER};
use crate::navidrome::{
    LinkState, NavidromeEvent, NavidromeService, ServiceOptions, NOT_CONNECTED,
};
use crate::secret::Secret;
use crate::settings::SettingsStore;

const WAIT: Duration = Duration::from_secs(15);
/// `STARRED_AT` of the fake server, 2026-01-01T12:00:00Z.
const STARRED_AT_MS: i64 = 1_767_268_800_000;

fn server() -> FakeServer {
    FakeServer::start(FakeLibrary::synthetic(4, 3))
}

fn client(server: &FakeServer) -> SubsonicClient {
    SubsonicClient::new(server.account()).unwrap()
}

fn db() -> Db {
    Db::open_in_memory().unwrap()
}

fn star_requests(server: &FakeServer) -> Vec<(String, Vec<(String, String)>)> {
    server
        .requests()
        .into_iter()
        .filter(|r| r.method == "star" || r.method == "unstar")
        .map(|r| {
            let params = r
                .params
                .into_iter()
                .filter(|(k, _)| k == "id" || k == "albumId" || k == "artistId")
                .collect();
            (r.method, params)
        })
        .collect()
}

fn heart(db: &Db, kind: FavKind, id: &str, on: bool, now: i64) -> bool {
    set_favorite(&lock(db).unwrap(), kind, id, on, now).unwrap()
}

fn ids(db: &Db) -> FavoriteIds {
    favorites::ids(&lock(db).unwrap()).unwrap()
}

fn queue(db: &Db) -> Vec<PendingStar> {
    pending(&lock(db).unwrap()).unwrap()
}

fn import_from(db: &Db, client: &SubsonicClient, now: i64) -> bool {
    let starred = client.starred2().unwrap();
    import(&mut lock(db).unwrap(), &starred, now).unwrap()
}

fn added_at(db: &Db, id: &str) -> i64 {
    lock(db)
        .unwrap()
        .query_row(
            "SELECT added_at FROM favorites WHERE item_id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap()
}

fn source_of(db: &Db, id: &str) -> String {
    lock(db)
        .unwrap()
        .query_row(
            "SELECT source FROM favorites WHERE item_id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn heart_writes_locally_and_queues() {
    let db = db();
    assert!(heart(&db, FavKind::Track, "navidrome:tr-1", true, 1_000));
    assert_eq!(ids(&db).tracks, ["navidrome:tr-1"]);
    assert_eq!(source_of(&db, "navidrome:tr-1"), "navidrome");
    assert_eq!(
        queue(&db),
        [PendingStar {
            kind: FavKind::Track,
            item_id: "navidrome:tr-1".into(),
            star: true,
            attempts: 0,
            queued_at: 1_000,
        }]
    );
    // Hearting again changes nothing locally; the intent stays one row.
    assert!(!heart(&db, FavKind::Track, "navidrome:tr-1", true, 2_000));
    assert_eq!(queue(&db).len(), 1);
    // A local id is not a Navidrome item.
    assert!(set_favorite(&lock(&db).unwrap(), FavKind::Track, "local:x", true, 1).is_err());
    assert!(set_favorite(&lock(&db).unwrap(), FavKind::Track, "navidrome:", true, 1).is_err());
}

#[test]
fn flush_sends_star_and_clears_queue() {
    let server = server();
    let client = client(&server);
    let db = db();
    heart(&db, FavKind::Track, "navidrome:tr-1", true, 1_000);
    let outcome = flush(&db, &client);
    assert_eq!(
        outcome,
        FlushOutcome {
            sent: 1,
            dropped: 0,
            offline: false
        }
    );
    assert!(server.starred().0.contains("tr-1"));
    assert!(queue(&db).is_empty());
    assert_eq!(ids(&db).tracks, ["navidrome:tr-1"]);
    // An empty queue sends nothing.
    assert_eq!(flush(&db, &client), FlushOutcome::default());
    assert_eq!(star_requests(&server).len(), 1);
}

#[test]
fn album_and_artist_map_to_album_id_and_artist_id() {
    assert_eq!(
        target(FavKind::Track, "navidrome:tr-1"),
        Some(StarTarget::Song("tr-1".into()))
    );
    assert_eq!(
        target(FavKind::Album, "navidrome:al-1"),
        Some(StarTarget::Album("al-1".into()))
    );
    assert_eq!(
        target(FavKind::Artist, "navidrome:ar-0"),
        Some(StarTarget::Artist("ar-0".into()))
    );

    let server = server();
    let client = client(&server);
    let db = db();
    heart(&db, FavKind::Album, "navidrome:al-1", true, 1);
    heart(&db, FavKind::Artist, "navidrome:ar-0", true, 2);
    assert_eq!(flush(&db, &client).sent, 2);
    assert_eq!(
        star_requests(&server),
        [
            ("star".to_string(), vec![("albumId".into(), "al-1".into())]),
            ("star".to_string(), vec![("artistId".into(), "ar-0".into())]),
        ]
    );
    let (songs, albums, artists) = server.starred();
    assert!(songs.is_empty());
    assert!(albums.contains("al-1"));
    assert!(artists.contains("ar-0"));
}

#[test]
fn local_favorites_never_reach_the_server() {
    // Local artists are keyed by name, local tracks and albums by their local ids.
    assert_eq!(target(FavKind::Artist, "Massive Attack"), None);
    assert_eq!(target(FavKind::Track, "local:t1"), None);
    assert_eq!(target(FavKind::Album, "local:album"), None);

    let server = server();
    let client = client(&server);
    let db = db();
    {
        let conn = lock(&db).unwrap();
        favorites::set_changed(&conn, FavKind::Track, "local:t1", true).unwrap();
        favorites::set_changed(&conn, FavKind::Artist, "Massive Attack", true).unwrap();
    }
    assert!(queue(&db).is_empty());
    assert_eq!(flush(&db, &client), FlushOutcome::default());
    assert!(star_requests(&server).is_empty());
    // The import leaves them alone too.
    import_from(&db, &client, 5);
    assert_eq!(ids(&db).tracks, ["local:t1"]);
    assert_eq!(ids(&db).artists, ["Massive Attack"]);
}

#[test]
fn toggle_twice_offline_sends_only_the_last_intent() {
    let server = server();
    let client = client(&server);
    let db = db();
    heart(&db, FavKind::Track, "navidrome:tr-2", true, 1);
    heart(&db, FavKind::Track, "navidrome:tr-2", false, 2);
    assert!(ids(&db).tracks.is_empty());

    server.set_fault(Fault::Refuse);
    let outcome = flush(&db, &client);
    assert!(outcome.offline);
    assert_eq!((outcome.sent, outcome.dropped), (0, 0));
    let queued = queue(&db);
    assert_eq!(queued.len(), 1);
    assert!(!queued[0].star);
    assert_eq!(queued[0].attempts, 1);

    server.set_fault(Fault::None);
    assert_eq!(flush(&db, &client).sent, 1);
    assert_eq!(
        star_requests(&server),
        [("unstar".to_string(), vec![("id".into(), "tr-2".into())])]
    );
    assert!(queue(&db).is_empty());
}

#[test]
fn offline_stops_the_flush_and_keeps_the_rest() {
    let server = server();
    let client = client(&server);
    let db = db();
    heart(&db, FavKind::Track, "navidrome:tr-1", true, 1);
    heart(&db, FavKind::Track, "navidrome:tr-2", true, 2);
    server.set_fault(Fault::Status(503));
    let outcome = flush(&db, &client);
    assert!(outcome.offline);
    // The first request failed; the second was never tried.
    assert_eq!(queue(&db).len(), 2);
    assert_eq!(queue(&db)[0].attempts, 1);
    assert_eq!(queue(&db)[1].attempts, 0);
}

#[test]
fn account_errors_keep_the_queue() {
    let server = server();
    let client = client(&server);
    let db = db();
    heart(&db, FavKind::Track, "navidrome:tr-1", true, 1);
    heart(&db, FavKind::Track, "navidrome:tr-2", true, 2);
    server.set_fault(Fault::WrongPassword);
    let outcome = flush(&db, &client);
    assert_eq!(outcome, FlushOutcome::default());
    assert_eq!(queue(&db).len(), 2);
    assert_eq!(ids(&db).tracks.len(), 2);
}

#[test]
fn deleted_item_drops_the_queue_entry() {
    let server = server();
    let client = client(&server);
    let db = db();
    heart(&db, FavKind::Track, "navidrome:gone", true, 1);
    heart(&db, FavKind::Album, "navidrome:gone-album", false, 2);
    server.set_fault(Fault::Code(70));
    let outcome = flush(&db, &client);
    assert_eq!(
        outcome,
        FlushOutcome {
            sent: 0,
            dropped: 2,
            offline: false
        }
    );
    assert!(queue(&db).is_empty());
    // The star of an item the server does not know cannot be kept.
    assert!(ids(&db).tracks.is_empty());
}

#[test]
fn other_server_errors_drop_the_entry_and_keep_the_favorite() {
    let server = server();
    let client = client(&server);
    let db = db();
    heart(&db, FavKind::Track, "navidrome:tr-1", true, 1);
    server.set_fault(Fault::Code(0));
    let outcome = flush(&db, &client);
    assert_eq!(outcome.dropped, 1);
    assert!(queue(&db).is_empty());
    // The next import sets it straight.
    assert_eq!(ids(&db).tracks, ["navidrome:tr-1"]);
}

#[test]
fn a_newer_intent_outlives_the_sent_one() {
    let db = db();
    heart(&db, FavKind::Track, "navidrome:tr-1", true, 1);
    let sent = queue(&db).remove(0);
    // The user unhearts while the star is on its way.
    heart(&db, FavKind::Track, "navidrome:tr-1", false, 2);
    assert!(!clear_sent(&lock(&db).unwrap(), &sent).unwrap());
    let queued = queue(&db);
    assert_eq!(queued.len(), 1);
    assert!(!queued[0].star);
}

#[test]
fn server_stars_are_imported() {
    let server = server();
    server.edit(|lib| {
        lib.starred_songs.insert("tr-1".into());
        lib.starred_albums.insert("al-2".into());
        lib.starred_artists.insert("ar-0".into());
    });
    let client = client(&server);
    let db = db();
    assert!(import_from(&db, &client, 99));
    let all = ids(&db);
    assert_eq!(all.tracks, ["navidrome:tr-1"]);
    assert_eq!(all.albums, ["navidrome:al-2"]);
    assert_eq!(all.artists, ["navidrome:ar-0"]);
    for id in ["navidrome:tr-1", "navidrome:al-2", "navidrome:ar-0"] {
        assert_eq!(source_of(&db, id), "navidrome");
        assert_eq!(added_at(&db, id), STARRED_AT_MS);
    }
    // Nothing new the second time.
    assert!(!import_from(&db, &client, 100));
}

#[test]
fn a_star_without_a_timestamp_is_added_now() {
    let db = db();
    let starred = Starred2 {
        song: vec![Child {
            id: "tr-9".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(import(&mut lock(&db).unwrap(), &starred, 777).unwrap());
    assert_eq!(added_at(&db, "navidrome:tr-9"), 777);
}

#[test]
fn server_unstar_removes_local_favorite() {
    let server = server();
    server.edit(|lib| {
        lib.starred_songs.insert("tr-1".into());
        lib.starred_songs.insert("tr-2".into());
    });
    let client = client(&server);
    let db = db();
    favorites::set_changed(&lock(&db).unwrap(), FavKind::Track, "local:t1", true).unwrap();
    import_from(&db, &client, 1);
    assert_eq!(ids(&db).tracks.len(), 3);

    server.edit(|lib| {
        lib.starred_songs.remove("tr-1");
    });
    assert!(import_from(&db, &client, 2));
    let mut tracks = ids(&db).tracks;
    tracks.sort();
    assert_eq!(tracks, ["local:t1", "navidrome:tr-2"]);
}

#[test]
fn pending_local_change_wins_over_server() {
    let server = server();
    server.edit(|lib| {
        lib.starred_songs.insert("tr-1".into());
        lib.starred_songs.insert("tr-3".into());
    });
    let client = client(&server);
    let db = db();
    import_from(&db, &client, 1);

    // Offline: unheart tr-1, heart tr-2. Meanwhile tr-3 is unstarred on another device.
    heart(&db, FavKind::Track, "navidrome:tr-1", false, 2);
    heart(&db, FavKind::Track, "navidrome:tr-2", true, 3);
    server.edit(|lib| {
        lib.starred_songs.remove("tr-3");
    });
    assert!(import_from(&db, &client, 4));
    // tr-1 stays unstarred, tr-2 stays starred, tr-3 follows the server.
    assert_eq!(ids(&db).tracks, ["navidrome:tr-2"]);
    assert_eq!(queue(&db).len(), 2);

    // Once sent, the server agrees and the import keeps it so.
    assert_eq!(flush(&db, &client).sent, 2);
    assert!(!import_from(&db, &client, 5));
    assert_eq!(ids(&db).tracks, ["navidrome:tr-2"]);
}

// The service: kick, retry, restart.

fn options() -> ServiceOptions {
    ServiceOptions {
        first_delay: Duration::from_millis(20),
        interval: Duration::from_secs(3600),
        star_retry: [
            Duration::from_millis(100),
            Duration::from_millis(500),
            Duration::from_secs(3),
        ],
        ..ServiceOptions::default()
    }
}

struct Service {
    service: Arc<NavidromeService>,
    db: Arc<Db>,
    events: Arc<Mutex<Vec<NavidromeEvent>>>,
}

/// A service over `db_path` (a fresh in-memory database when `None`) with the fake account
/// saved in `dir`.
fn service(dir: &Path, db_path: Option<&Path>, creds: &Arc<MemoryStore>) -> Service {
    let db = Arc::new(match db_path {
        Some(path) => Db::open(path).unwrap(),
        None => db(),
    });
    let settings = Arc::new(SettingsStore::load(&dir.join("settings.json")));
    let events: Arc<Mutex<Vec<NavidromeEvent>>> = Arc::default();
    let recorded = Arc::clone(&events);
    let service = NavidromeService::spawn_with_options(
        Arc::clone(&db),
        settings,
        Arc::clone(creds) as Arc<dyn CredentialStore>,
        Arc::new(move |event| recorded.lock().unwrap().push(event)),
        options(),
    );
    Service {
        service,
        db,
        events,
    }
}

fn save_account(dir: &Path, server: &FakeServer, creds: &MemoryStore) {
    SettingsStore::load(&dir.join("settings.json"))
        .set_navidrome_account(Some((server.url(), FAKE_USER.into())))
        .unwrap();
    creds
        .set(&server.url(), FAKE_USER, &Secret::new(FAKE_PASSWORD))
        .unwrap();
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Service {
    fn wait_online(&self) {
        wait_for("Online", || {
            self.service.status().state == LinkState::Online
        });
    }

    fn favorites_events(&self) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, NavidromeEvent::FavoritesChanged))
            .count()
    }
}

#[test]
fn hearting_while_signed_out_is_refused() {
    let dir = TempDir::new().unwrap();
    let creds = Arc::new(MemoryStore::new());
    let env = service(dir.path(), None, &creds);
    let err = env
        .service
        .set_favorite(FavKind::Track, "navidrome:tr-1", true)
        .unwrap_err();
    assert_eq!(err.to_string(), NOT_CONNECTED);
    // `favorites_set` sends `{err:#}`; the frontend matches this exact text.
    assert_eq!(format!("{err:#}"), NOT_CONNECTED);
    assert_eq!(NOT_CONNECTED, "Navidrome is not connected");
    assert!(queue(&env.db).is_empty());
    assert!(ids(&env.db).tracks.is_empty());
}

#[test]
fn heart_is_sent_right_away() {
    let server = server();
    let dir = TempDir::new().unwrap();
    let creds = Arc::new(MemoryStore::new());
    save_account(dir.path(), &server, &creds);
    let env = service(dir.path(), None, &creds);
    env.wait_online();
    assert!(env
        .service
        .set_favorite(FavKind::Album, "navidrome:al-1", true)
        .unwrap());
    // No cycle runs (the interval is an hour): the kick sends it.
    wait_for("the star", || server.starred().1.contains("al-1"));
    wait_for("an empty queue", || queue(&env.db).is_empty());
}

#[test]
fn offline_kick_is_retried() {
    let server = server();
    let dir = TempDir::new().unwrap();
    let creds = Arc::new(MemoryStore::new());
    save_account(dir.path(), &server, &creds);
    let env = service(dir.path(), None, &creds);
    env.wait_online();
    server.set_fault(Fault::Status(502));
    env.service
        .set_favorite(FavKind::Track, "navidrome:tr-4", true)
        .unwrap();
    wait_for("a failed attempt", || {
        queue(&env.db).first().is_some_and(|p| p.attempts >= 1)
    });
    server.set_fault(Fault::None);
    wait_for("the retried star", || server.starred().0.contains("tr-4"));
    wait_for("an empty queue", || queue(&env.db).is_empty());
}

#[test]
fn unstar_while_offline_survives_restart_and_flushes() {
    let server = server();
    server.edit(|lib| {
        lib.starred_songs.insert("tr-1".into());
    });
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    let creds = Arc::new(MemoryStore::new());
    save_account(dir.path(), &server, &creds);
    {
        let env = service(dir.path(), Some(&db_path), &creds);
        env.wait_online();
        assert_eq!(ids(&env.db).tracks, ["navidrome:tr-1"]);
        assert!(env.favorites_events() >= 1);

        server.set_fault(Fault::Refuse);
        assert!(env
            .service
            .set_favorite(FavKind::Track, "navidrome:tr-1", false)
            .unwrap());
        wait_for("a failed attempt", || {
            queue(&env.db).first().is_some_and(|p| p.attempts >= 1)
        });
        // Quit while the server is still away.
        env.service.shutdown();
    }
    // Meanwhile another device stars tr-2.
    server.set_fault(Fault::None);
    server.edit(|lib| {
        lib.starred_songs.insert("tr-2".into());
    });

    {
        let reopened = Db::open(&db_path).unwrap();
        let queued = queue(&reopened);
        assert_eq!(queued.len(), 1, "the intent survived the restart");
        assert!(!queued[0].star);
    }
    let env = service(dir.path(), Some(&db_path), &creds);
    env.wait_online();
    wait_for("the unstar", || !server.starred().0.contains("tr-1"));
    assert!(queue(&env.db).is_empty());
    assert_eq!(ids(&env.db).tracks, ["navidrome:tr-2"]);
}

#[test]
fn cycle_imports_and_reports_changes() {
    let server = server();
    server.edit(|lib| {
        lib.starred_artists.insert("ar-1".into());
    });
    let dir = TempDir::new().unwrap();
    let creds = Arc::new(MemoryStore::new());
    save_account(dir.path(), &server, &creds);
    let env = service(dir.path(), None, &creds);
    env.wait_online();
    assert_eq!(ids(&env.db).artists, ["navidrome:ar-1"]);
    assert_eq!(env.favorites_events(), 1);
    // A cycle without changes says nothing.
    env.service.sync_now();
    assert!(env.service.wait_idle(WAIT));
    let imports = server
        .requests()
        .iter()
        .filter(|r| r.method == "getStarred2")
        .count();
    assert_eq!(imports, 2);
    assert_eq!(env.favorites_events(), 1);
}

/// Starts a service against `server` with `fault` set and waits for its first cycle.
fn first_cycle_with(server: &FakeServer, fault: Fault) -> (TempDir, Service) {
    server.set_fault(fault);
    let dir = TempDir::new().unwrap();
    let creds = Arc::new(MemoryStore::new());
    save_account(dir.path(), server, &creds);
    let env = service(dir.path(), None, &creds);
    wait_for("the first cycle", || {
        server.requests().iter().any(|r| r.method == "getStarred2")
    });
    assert!(env.service.wait_idle(WAIT));
    (dir, env)
}

fn nd_tracks(db: &Db) -> i64 {
    lock(db)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE source = 'navidrome'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn a_failed_star_import_does_not_halt_the_cycle() {
    let faults = [
        Fault::CodeOn {
            method: "getStarred2",
            code: 0,
        },
        Fault::CodeOn {
            method: "getStarred2",
            code: 50,
        },
        Fault::Payload {
            method: "getStarred2",
            payload: json!({}),
        },
    ];
    for fault in faults {
        let server = server();
        let (_dir, env) = first_cycle_with(&server, fault.clone());
        let status = env.service.status();
        assert_eq!(status.state, LinkState::Online, "{fault:?}");
        assert_eq!(status.error, None, "{fault:?}");
        assert!(status.last_sync.is_some(), "{fault:?}");
        assert_eq!(nd_tracks(&env.db), 12, "{fault:?}");
        assert!(ids(&env.db).tracks.is_empty());
    }
}

#[test]
fn an_account_error_on_the_star_import_ends_the_cycle() {
    let server = server();
    let (_dir, env) = first_cycle_with(
        &server,
        Fault::CodeOn {
            method: "getStarred2",
            code: 40,
        },
    );
    let status = env.service.status();
    assert_eq!(status.state, LinkState::Error);
    assert_eq!(status.error.unwrap().code, "wrongCredentials");
}

#[test]
fn an_offline_flush_ends_the_cycle_early() {
    let server = server();
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    {
        // Queued for this account, as a heart of an earlier run would be.
        let db = Db::open(&db_path).unwrap();
        let account = client(&server).account().clone();
        ensure_server(&db, &server_key(&account.url, &account.user)).unwrap();
        heart(&db, FavKind::Track, "navidrome:tr-1", true, 1);
    }
    let creds = Arc::new(MemoryStore::new());
    save_account(dir.path(), &server, &creds);
    server.set_fault(Fault::Status(503));
    let env = service(dir.path(), Some(&db_path), &creds);
    wait_for("Offline", || {
        env.service.status().state == LinkState::Offline
    });
    // The flush tried; the library step never asked the server.
    assert!(server.requests().iter().any(|r| r.method == "star"));
    assert!(server.requests().iter().all(|r| r.method != "getIndexes"));
    assert_eq!(queue(&env.db).len(), 1);
}

#[test]
fn the_wipe_takes_every_navidrome_favorite() {
    let db = db();
    {
        let conn = lock(&db).unwrap();
        // A Navidrome id written with the local source (before stars carried theirs).
        favorites::set_changed(&conn, FavKind::Track, "navidrome:tr-1", true).unwrap();
        favorites::set_changed(&conn, FavKind::Track, "local:t1", true).unwrap();
    }
    heart(&db, FavKind::Album, "navidrome:al-1", true, 1);
    {
        let mut conn = lock(&db).unwrap();
        let tx = conn.transaction().unwrap();
        wipe_navidrome(&tx).unwrap();
        tx.commit().unwrap();
    }
    let all = ids(&db);
    assert_eq!(all.tracks, ["local:t1"]);
    assert!(all.albums.is_empty());
    assert!(queue(&db).is_empty());
}
