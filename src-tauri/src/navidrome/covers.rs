//! Server covers for the resize gate: `getCoverArt` of the signed-in account.
//!
//! The gate asks for an `nd-` cover at the size it serves (96, 300 or 600) and caches the
//! result like a local cover. While the server does not answer with these credentials, no
//! request goes out at all: the cover is `Unreachable` (503, not cached) and asked again later.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::client::NavidromeError;
use super::NavidromeService;
use crate::library::model::NAVIDROME_COVER_PREFIX;
use crate::thumbs::{RemoteCovers, ResizeError, ResizeGate};

/// `getCoverArt` through the service's current client.
pub struct ServerCovers(pub Arc<NavidromeService>);

impl RemoteCovers for ServerCovers {
    fn fetch(&self, cover_art: &str, size: u32) -> Result<Vec<u8>, ResizeError> {
        // Off, offline or a credential error: no request.
        if !self.0.online_flag().load(Ordering::SeqCst) {
            return Err(ResizeError::Unreachable);
        }
        let Some(client) = self.0.client() else {
            return Err(ResizeError::Unreachable);
        };
        // The error itself is not logged: it adds nothing the status does not show.
        match client.cover_art(cover_art, size) {
            Ok(bytes) => Ok(bytes),
            Err(NavidromeError::NotFound) => Err(ResizeError::Unknown),
            Err(_) => Err(ResizeError::Unreachable),
        }
    }
}

/// Wires the gate and the service together: `nd-` covers are fetched from the server, and
/// every wipe (server switch, disconnect) purges the cached server covers under `cache`.
/// The purge holds the gate weakly, so the two do not keep each other alive.
pub fn attach(gate: &Arc<ResizeGate>, service: &Arc<NavidromeService>, cache: PathBuf) {
    gate.set_remote_for(
        NAVIDROME_COVER_PREFIX,
        Arc::new(ServerCovers(Arc::clone(service))),
    );
    let weak = Arc::downgrade(gate);
    service.set_cover_purge(Arc::new(move || {
        if let Some(gate) = weak.upgrade() {
            gate.purge_remote(&cache, NAVIDROME_COVER_PREFIX);
        }
    }));
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::super::keychain::{CredentialStore, MemoryStore};
    use super::super::sync::{ensure_server, server_key};
    use super::super::testserver::{FakeLibrary, FakeServer, FAKE_PASSWORD, FAKE_USER};
    use super::super::{LinkState, ServiceOptions};
    use super::*;
    use crate::db::Db;
    use crate::library::covers::CoverStore;
    use crate::secret::Secret;
    use crate::settings::SettingsStore;

    const WAIT: Duration = Duration::from_secs(15);

    struct Env {
        dir: TempDir,
        db: Arc<Db>,
        service: Arc<NavidromeService>,
        gate: Arc<ResizeGate>,
        store: CoverStore,
    }

    impl Env {
        fn cache(&self) -> &Path {
            self.dir.path()
        }

        fn cover(&self, id: &str, size: u32) -> Result<PathBuf, ResizeError> {
            self.gate.get(&self.store, self.cache(), id, size)
        }

        fn connect(&self, server: &FakeServer, user: &str) {
            self.service
                .connect(&server.url(), user, Secret::new(FAKE_PASSWORD))
                .unwrap();
        }

        fn wait_state(&self, state: LinkState) {
            wait_for(&format!("{state:?}"), || {
                self.service.status().state == state
            });
        }
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn options() -> ServiceOptions {
        ServiceOptions {
            first_delay: Duration::from_millis(20),
            interval: Duration::from_secs(3600),
            offline_retry: Duration::from_secs(3600),
            ..ServiceOptions::default()
        }
    }

    /// The service and a gate wired with [`attach`]; `prepare` may fill the database and save
    /// an account before the service starts.
    fn start(prepare: impl FnOnce(&Path, &Db, &MemoryStore)) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let creds = Arc::new(MemoryStore::new());
        prepare(&settings_path, &db, &creds);
        let service = NavidromeService::spawn_with_options(
            Arc::clone(&db),
            Arc::new(SettingsStore::load(&settings_path)),
            creds as Arc<dyn CredentialStore>,
            Arc::new(|_| {}),
            options(),
        );
        let gate = Arc::new(ResizeGate::new(2));
        attach(&gate, &service, dir.path().to_path_buf());
        let store = CoverStore::new(dir.path().join("original"));
        Env {
            dir,
            db,
            service,
            gate,
            store,
        }
    }

    fn cover_requests(server: &FakeServer) -> Vec<Vec<(String, String)>> {
        server
            .requests()
            .into_iter()
            .filter(|r| r.method == "getCoverArt")
            .map(|r| r.params)
            .collect()
    }

    fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn nd_files(cache: &Path) -> usize {
        crate::thumbs::SIZES
            .iter()
            .filter_map(|size| std::fs::read_dir(cache.join(size.to_string())).ok())
            .flat_map(|entries| entries.flatten())
            .filter(|e| e.file_name().to_string_lossy().starts_with("nd-"))
            .count()
    }

    #[test]
    fn remote_cover_is_requested_at_the_needed_size() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let env = start(|_, _, _| {});
        env.connect(&server, FAKE_USER);
        env.wait_state(LinkState::Online);

        let path = env.cover("nd-al-0_cover", 300).unwrap();
        assert_eq!(path, env.cache().join("300").join("nd-al-0_cover.jpg"));
        let decoded = image::load_from_memory(&std::fs::read(&path).unwrap()).unwrap();
        assert!(decoded.width() <= 300 && decoded.height() <= 300);
        let requests = cover_requests(&server);
        assert_eq!(requests.len(), 1);
        assert_eq!(param(&requests[0], "id"), Some("al-0_cover"));
        assert_eq!(param(&requests[0], "size"), Some("300"));

        // A cover the server does not know is unknown, and nothing is written.
        assert_eq!(env.cover("nd-missing", 96), Err(ResizeError::Unknown));
        assert!(!env.cache().join("96").join("nd-missing.jpg").exists());
    }

    #[test]
    fn offline_service_makes_no_cover_requests() {
        // No account at all.
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let env = start(|_, _, _| {});
        assert_eq!(
            env.cover("nd-al-0_cover", 300),
            Err(ResizeError::Unreachable)
        );
        assert!(server.requests().is_empty());

        env.connect(&server, FAKE_USER);
        env.wait_state(LinkState::Online);
        server.stop();
        env.service.sync_now();
        env.wait_state(LinkState::Offline);
        let before = server.requests().len();
        for size in crate::thumbs::SIZES {
            assert_eq!(
                env.cover("nd-al-0_cover", size),
                Err(ResizeError::Unreachable)
            );
        }
        assert_eq!(server.requests().len(), before, "no request while offline");
        assert_eq!(nd_files(env.cache()), 0, "nothing cached");
    }

    #[test]
    fn switching_server_purges_cached_covers() {
        let first = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let env = start(|_, _, _| {});
        env.connect(&first, FAKE_USER);
        env.wait_state(LinkState::Online);
        env.cover("nd-al-0_cover", 96).unwrap();
        env.cover("nd-al-0_cover", 600).unwrap();
        let jpeg = std::fs::read(env.cover("nd-al-0_cover", 300).unwrap()).unwrap();
        let local = env.store.store_bytes(&jpeg).unwrap();
        let local_file = env.cover(&local, 300).unwrap();
        assert_eq!(nd_files(env.cache()), 3);

        // The same account again: nothing is purged.
        env.connect(&first, FAKE_USER);
        assert_eq!(nd_files(env.cache()), 3);

        // Another server is another account.
        let second = FakeServer::start(FakeLibrary::synthetic(1, 1));
        env.connect(&second, FAKE_USER);
        assert_eq!(nd_files(env.cache()), 0);
        assert!(local_file.is_file(), "local covers stay");
    }

    #[test]
    fn disconnect_purges_cached_covers() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let env = start(|_, _, _| {});
        env.connect(&server, FAKE_USER);
        env.wait_state(LinkState::Online);
        env.cover("nd-al-0_cover", 300).unwrap();
        {
            let conn = env.db.0.lock().unwrap();
            conn.execute_batch(
                "INSERT INTO covers (id, file) VALUES ('nd-al-0_cover', 'x.jpg');
                 INSERT INTO cover_lookups (album_id, status, checked_at)
                     VALUES ('navidrome:al-0', 'notfound', 1);",
            )
            .unwrap();
        }
        env.service.disconnect().unwrap();
        assert_eq!(nd_files(env.cache()), 0);
        let conn = env.db.0.lock().unwrap();
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT COUNT(*) FROM covers WHERE id GLOB 'nd-*'"), 0);
        assert_eq!(
            count("SELECT COUNT(*) FROM cover_lookups WHERE album_id GLOB 'navidrome:*'"),
            0
        );
    }

    #[test]
    fn a_wipe_at_start_purges_once_the_gate_is_attached() {
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let url = server.url();
        let calls = Arc::new(AtomicUsize::new(0));
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        let db = Arc::new(Db::open_in_memory().unwrap());
        // The index belongs to another account; the saved one is new.
        ensure_server(&db, &server_key(&url, "someone-else")).unwrap();
        SettingsStore::load(&settings_path)
            .set_navidrome_account(Some((url.clone(), FAKE_USER.into())))
            .unwrap();
        let creds = Arc::new(MemoryStore::new());
        creds
            .set(&url, FAKE_USER, &Secret::new(FAKE_PASSWORD))
            .unwrap();
        let service = NavidromeService::spawn_with_options(
            Arc::clone(&db),
            Arc::new(SettingsStore::load(&settings_path)),
            creds as Arc<dyn CredentialStore>,
            Arc::new(|_| {}),
            options(),
        );
        let counted = Arc::clone(&calls);
        service.set_cover_purge(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Plugging it in again does not purge again.
        let counted = Arc::clone(&calls);
        service.set_cover_purge(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        service.shutdown();
    }

    #[test]
    fn purge_hook_runs_outside_the_service_locks() {
        // The hook may take the database lock and read the status: neither is held.
        let server = FakeServer::start(FakeLibrary::synthetic(1, 1));
        let env = start(|_, _, _| {});
        env.connect(&server, FAKE_USER);
        env.wait_state(LinkState::Online);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (db, service, recorded) = (
            Arc::clone(&env.db),
            Arc::downgrade(&env.service),
            Arc::clone(&seen),
        );
        env.service.set_cover_purge(Arc::new(move || {
            let tracks: i64 =
                db.0.try_lock()
                    .expect("the database lock is free")
                    .query_row(
                        "SELECT COUNT(*) FROM tracks WHERE source = 'navidrome'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
            if let Some(service) = service.upgrade() {
                let _ = service.status();
            }
            recorded.lock().unwrap().push(tracks);
        }));
        env.service.disconnect().unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            vec![0],
            "purged after the wipe committed"
        );
    }
}
