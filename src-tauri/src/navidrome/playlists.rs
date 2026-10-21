//! The server's playlists, read-only: `remote_playlists` and `remote_playlist_tracks`.
//!
//! Every cycle lists the playlists (`getPlaylists`, one call) and fetches the songs
//! (`getPlaylist`) only of a playlist that is new or whose `changed` value differs from the
//! stored one. Playlists the server no longer lists are deleted. Network calls run outside the
//! database lock; the whole result is written in one transaction, after the cancel check under
//! the lock, so a wipe that holds the lock is never followed by rows of the old server.
//!
//! An entry keeps its track id also when the track is not in the index (not synced yet, or
//! gone from the server); it comes back with `track: None` and resolves when the track returns.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;

use super::client::{NavidromeError, PlaylistSummary, PlaylistWithSongs, SubsonicClient};
use super::sync::SyncError;
use crate::db::Db;
use crate::library::model::navidrome_id;
use crate::library::playlists::{entry_totals, resolve_entries, EntryTotals, PlaylistEntry};
use crate::library::scan::lock;

/// A playlist of the server as the UI lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylist {
    /// `navidrome:<playlist id>`.
    pub id: String,
    pub name: String,
    pub owner: Option<String>,
    /// The server's song count.
    pub track_count: u32,
    /// The server's total duration.
    pub duration_ms: u64,
    /// Up to four distinct cover ids of indexed tracks, in playlist order.
    pub cover_ids: Vec<String>,
    /// The server's `changed` time (ms), or when Prism first stored the playlist.
    pub updated_at: i64,
}

/// What a fetched playlist writes.
struct Fetched {
    summary: PlaylistSummary,
    track_ids: Vec<String>,
}

/// A stored playlist: its `changed` value and song count, for the refetch decision.
struct Stored {
    changed: Option<String>,
    track_count: u32,
}

/// Whether a listed playlist needs its songs fetched: new, `changed` differs or is missing
/// (a server without it is fetched every cycle), or the song count moved.
fn needs_fetch(listed: &PlaylistSummary, stored: Option<&Stored>) -> bool {
    match stored {
        None => true,
        Some(stored) => {
            listed.changed.is_none()
                || listed.changed != stored.changed
                || listed.song_count != stored.track_count
        }
    }
}

/// Syncs the server's playlists. Returns whether anything in the index changed.
pub fn sync_playlists(db: &Db, client: &SubsonicClient) -> Result<bool, SyncError> {
    sync_playlists_cancellable(db, client, now_ms(), &|| false)
}

/// [`sync_playlists`] that stops with [`SyncError::Cancelled`] when `cancelled` returns true.
/// `cancelled` is checked under the database lock right before the write.
pub fn sync_playlists_cancellable(
    db: &Db,
    client: &SubsonicClient,
    now_ms: i64,
    cancelled: &dyn Fn() -> bool,
) -> Result<bool, SyncError> {
    let listed: Vec<PlaylistSummary> = client
        .playlists()?
        .into_iter()
        .filter(|p| !p.id.is_empty())
        .collect();
    let stored = stored_playlists(&*lock(db)?)?;

    let mut fetched: Vec<Fetched> = Vec::new();
    let mut vanished: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for summary in &listed {
        if !needs_fetch(summary, stored.get(&navidrome_id(&summary.id))) {
            continue;
        }
        if cancelled() {
            return Err(SyncError::Cancelled);
        }
        match client.playlist(&summary.id) {
            Ok(PlaylistWithSongs {
                summary: full,
                entry,
            }) => fetched.push(Fetched {
                summary: PlaylistSummary {
                    // The id as listed: a server that echoes another id does not move it.
                    id: summary.id.clone(),
                    ..full
                },
                track_ids: entry
                    .iter()
                    .filter(|song| !song.id.is_empty())
                    .map(|song| navidrome_id(&song.id))
                    .collect(),
            }),
            // Deleted between the two calls.
            Err(NavidromeError::NotFound) => vanished.push(summary.id.clone()),
            // This one playlist failed: the rest syncs, it keeps its rows and is fetched
            // again next cycle.
            Err(err) if skips_playlist(&err) => {
                eprintln!("Navidrome playlist skipped: {}", err.code());
                skipped.push(summary.id.clone());
            }
            Err(err) => return Err(err.into()),
        }
    }

    let mut conn = lock(db)?;
    if cancelled() {
        return Err(SyncError::Cancelled);
    }
    let tx = conn.transaction()?;
    let rows = write(&tx, &listed, &fetched, &vanished, &skipped, now_ms)?;
    tx.commit()?;
    Ok(rows > 0)
}

/// Whether a `getPlaylist` failure concerns that playlist only (a server error, a refused or
/// unreadable answer), so the cycle goes on without it. Offline and account-level failures
/// (credentials, TLS, token auth, the address) end the cycle as before.
fn skips_playlist(err: &NavidromeError) -> bool {
    match err {
        NavidromeError::Server { .. }
        | NavidromeError::NotSubsonic(_)
        | NavidromeError::NotAuthorized => true,
        NavidromeError::InvalidUrl(_)
        | NavidromeError::WrongCredentials
        | NavidromeError::TokenAuthUnsupported
        | NavidromeError::NotFound
        | NavidromeError::Unreachable(_)
        | NavidromeError::Tls(_) => false,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn stored_playlists(conn: &Connection) -> rusqlite::Result<HashMap<String, Stored>> {
    let mut stmt = conn.prepare("SELECT id, changed, track_count FROM remote_playlists")?;
    let rows = stmt.query_map([], |row| {
        let count: i64 = row.get(2)?;
        Ok((
            row.get::<_, String>(0)?,
            Stored {
                changed: row.get(1)?,
                track_count: count.clamp(0, i64::from(u32::MAX)) as u32,
            },
        ))
    })?;
    rows.collect()
}

/// Upserts the summary of a playlist, only when a column differs; returns the rows changed.
/// `updated_at` is the server's `changed` time, or `now_ms` when it has none (or it does not
/// parse); an unparsable value keeps the time stored before.
fn upsert_summary(
    tx: &Transaction,
    summary: &PlaylistSummary,
    now_ms: i64,
) -> rusqlite::Result<usize> {
    tx.prepare_cached(
        "INSERT INTO remote_playlists (id, name, owner, changed, track_count, duration_ms,
             updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6,
             COALESCE(CAST(ROUND((julianday(?4) - 2440587.5) * 86400000) AS INTEGER), ?7))
         ON CONFLICT(id) DO UPDATE SET
             name = excluded.name,
             owner = excluded.owner,
             changed = excluded.changed,
             track_count = excluded.track_count,
             duration_ms = excluded.duration_ms,
             updated_at = CASE
                 WHEN julianday(excluded.changed) IS NULL THEN remote_playlists.updated_at
                 ELSE excluded.updated_at END
         WHERE remote_playlists.name IS NOT excluded.name
             OR remote_playlists.owner IS NOT excluded.owner
             OR remote_playlists.changed IS NOT excluded.changed
             OR remote_playlists.track_count IS NOT excluded.track_count
             OR remote_playlists.duration_ms IS NOT excluded.duration_ms",
    )?
    .execute(params![
        navidrome_id(&summary.id),
        summary.name,
        summary.owner,
        summary.changed,
        i64::from(summary.song_count),
        i64::from(summary.duration) * 1000,
        now_ms,
    ])
}

/// Writes one sync's result; returns the rows changed. A `skipped` playlist (its fetch failed)
/// is left exactly as stored: its listing summary is not written either, or its new `changed`
/// value would hide the change from the next cycle.
fn write(
    tx: &Transaction,
    listed: &[PlaylistSummary],
    fetched: &[Fetched],
    vanished: &[String],
    skipped: &[String],
    now_ms: i64,
) -> rusqlite::Result<usize> {
    let mut rows = 0;
    let fetched_ids: HashMap<&str, &Fetched> =
        fetched.iter().map(|f| (f.summary.id.as_str(), f)).collect();
    for summary in listed {
        if vanished.contains(&summary.id) || skipped.contains(&summary.id) {
            continue;
        }
        match fetched_ids.get(summary.id.as_str()) {
            Some(fetched) => {
                rows += upsert_summary(tx, &fetched.summary, now_ms)?;
                rows += write_entries(tx, &navidrome_id(&summary.id), &fetched.track_ids)?;
            }
            // Unchanged songs: the name or owner may still have moved.
            None => rows += upsert_summary(tx, summary, now_ms)?,
        }
    }
    // Everything the server no longer lists (entries go through the foreign key).
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS nd_listed_playlists (id TEXT PRIMARY KEY);
         DELETE FROM temp.nd_listed_playlists;",
    )?;
    {
        let mut insert =
            tx.prepare_cached("INSERT OR IGNORE INTO temp.nd_listed_playlists (id) VALUES (?1)")?;
        for summary in listed {
            if !vanished.contains(&summary.id) {
                insert.execute([navidrome_id(&summary.id)])?;
            }
        }
    }
    rows += tx.execute(
        "DELETE FROM remote_playlists WHERE id NOT IN (SELECT id FROM temp.nd_listed_playlists)",
        [],
    )?;
    tx.execute("DELETE FROM temp.nd_listed_playlists", [])?;
    Ok(rows)
}

/// Replaces the entries of a playlist when they differ; returns the rows changed.
fn write_entries(tx: &Transaction, id: &str, track_ids: &[String]) -> rusqlite::Result<usize> {
    let before: Vec<String> = {
        let mut stmt = tx.prepare_cached(
            "SELECT track_id FROM remote_playlist_tracks WHERE playlist_id = ?1 ORDER BY position",
        )?;
        let rows = stmt.query_map([id], |row| row.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    if before == track_ids {
        return Ok(0);
    }
    let mut rows = tx.execute(
        "DELETE FROM remote_playlist_tracks WHERE playlist_id = ?1",
        [id],
    )?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO remote_playlist_tracks (playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
    )?;
    for (position, track_id) in track_ids.iter().enumerate() {
        rows += insert.execute(params![id, position as i64, track_id])?;
    }
    Ok(rows)
}

/// Every server playlist, most recently changed first (like the Prism playlists): one query
/// for the playlists and one ordered scan of every entry for the covers. Count and duration
/// are the server's.
pub fn list(conn: &Connection) -> anyhow::Result<Vec<RemotePlaylist>> {
    let mut totals: HashMap<String, EntryTotals> = entry_totals(conn, "remote_playlist_tracks")?;
    let mut stmt = conn.prepare(
        "SELECT id, name, owner, track_count, duration_ms, updated_at FROM remote_playlists
         ORDER BY updated_at DESC, name COLLATE NOCASE, id",
    )?;
    let rows = stmt.query_map([], |row| {
        let track_count: i64 = row.get(3)?;
        let duration_ms: i64 = row.get(4)?;
        Ok(RemotePlaylist {
            id: row.get(0)?,
            name: row.get(1)?,
            owner: row.get(2)?,
            track_count: track_count.clamp(0, i64::from(u32::MAX)) as u32,
            duration_ms: duration_ms.max(0) as u64,
            cover_ids: Vec::new(),
            updated_at: row.get(5)?,
        })
    })?;
    let mut out = Vec::new();
    for playlist in rows {
        let mut playlist = playlist?;
        if let Some(totals) = totals.remove(&playlist.id) {
            playlist.cover_ids = totals.cover_ids;
        }
        out.push(playlist);
    }
    Ok(out)
}

/// The entries of a server playlist in order; a track that is not in the index comes back with
/// `track: None`.
pub fn entries(conn: &Connection, id: &str) -> anyhow::Result<Vec<PlaylistEntry>> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM remote_playlists WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    if found.is_none() {
        anyhow::bail!("This playlist no longer exists");
    }
    resolve_entries(conn, "remote_playlist_tracks", &id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::playlists::assert_scan_needs_no_sort;
    use crate::library::playlists::{
        playlist_add, playlist_create, playlist_entries, playlists_list,
    };
    use crate::navidrome::sync::{ensure_server, sync_library};
    use crate::navidrome::testserver::{FakeLibrary, FakeServer, Fault, FAKE_USER};

    struct Env {
        server: FakeServer,
        client: SubsonicClient,
        db: Db,
    }

    /// Two albums of five songs (`tr-0` to `tr-9`); `pl-0` holds `tr-0` to `tr-4`, `pl-1`
    /// holds `tr-0` and `tr-7`.
    fn env() -> Env {
        let server = FakeServer::start(FakeLibrary::synthetic(2, 5));
        let client = SubsonicClient::new(server.account()).unwrap();
        let db = Db::open_in_memory().unwrap();
        Env { server, client, db }
    }

    impl Env {
        fn library(&self) {
            sync_library(&self.db, &self.client, 1, &|_| {}, &|| {}).unwrap();
        }

        fn sync(&self) -> bool {
            sync_playlists(&self.db, &self.client).unwrap()
        }

        fn list(&self) -> Vec<RemotePlaylist> {
            list(&self.db.0.lock().unwrap()).unwrap()
        }

        fn entries(&self, id: &str) -> Vec<PlaylistEntry> {
            entries(&self.db.0.lock().unwrap(), id).unwrap()
        }

        fn count(&self, sql: &str) -> i64 {
            let conn = self.db.0.lock().unwrap();
            conn.query_row(sql, [], |row| row.get(0)).unwrap()
        }

        fn requests(&self, method: &str) -> usize {
            self.server
                .requests()
                .iter()
                .filter(|r| r.method == method)
                .count()
        }
    }

    fn ids(entries: &[PlaylistEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.track_id.as_str()).collect()
    }

    #[test]
    fn playlists_synced_with_entries() {
        let env = env();
        env.library();
        assert!(env.sync());
        let listed = env.list();
        assert_eq!(listed.len(), 2);
        let first = listed.iter().find(|p| p.id == "navidrome:pl-0").unwrap();
        assert_eq!(first.name, "Demo Playlist 1");
        assert_eq!(first.owner.as_deref(), Some(FAKE_USER));
        assert_eq!(first.track_count, 5);
        // 180 + 181 + 182 + 183 + 184 seconds.
        assert_eq!(first.duration_ms, 910_000);
        assert_eq!(first.cover_ids, vec!["nd-al-0_cover".to_string()]);
        // The server's `changed` time.
        assert_eq!(first.updated_at, 1_767_225_600_000);
        let second = listed.iter().find(|p| p.id == "navidrome:pl-1").unwrap();
        assert_eq!(
            second.cover_ids,
            vec!["nd-al-0_cover".to_string(), "nd-al-1_cover".to_string()]
        );

        let entries = env.entries("navidrome:pl-0");
        assert_eq!(
            ids(&entries),
            vec![
                "navidrome:tr-0",
                "navidrome:tr-1",
                "navidrome:tr-2",
                "navidrome:tr-3",
                "navidrome:tr-4"
            ]
        );
        assert!(entries.iter().all(|e| e.track.is_some()));
        assert_eq!(
            entries.iter().map(|e| e.position).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert_eq!(
            ids(&env.entries("navidrome:pl-1")),
            vec!["navidrome:tr-0", "navidrome:tr-7"]
        );
        assert!(super::entries(&env.db.0.lock().unwrap(), "navidrome:nope").is_err());
        // Nothing changed on the server: nothing changes here.
        assert!(!env.sync());
    }

    #[test]
    fn unchanged_playlist_not_refetched() {
        let env = env();
        env.sync();
        assert_eq!(env.requests("getPlaylists"), 1);
        assert_eq!(env.requests("getPlaylist"), 2);
        // Second sync: one listing, no playlist fetched.
        assert!(!env.sync());
        assert_eq!(env.requests("getPlaylists"), 2);
        assert_eq!(env.requests("getPlaylist"), 2);

        // One playlist changes: only that one is fetched.
        env.server.edit(|lib| {
            let playlist = &mut lib.playlists[1];
            playlist.summary.changed = Some("2026-02-01T00:00:00.123456789Z".into());
            playlist.song_ids.push("tr-9".into());
        });
        assert!(env.sync());
        assert_eq!(env.requests("getPlaylists"), 3);
        let fetched: Vec<String> = env
            .server
            .requests()
            .iter()
            .filter(|r| r.method == "getPlaylist")
            .skip(2)
            .flat_map(|r| {
                r.params
                    .iter()
                    .filter(|(k, _)| k == "id")
                    .map(|(_, v)| v.clone())
            })
            .collect();
        assert_eq!(fetched, vec!["pl-1".to_string()]);
        assert_eq!(
            ids(&env.entries("navidrome:pl-1")),
            vec!["navidrome:tr-0", "navidrome:tr-7", "navidrome:tr-9"]
        );
        let changed = env
            .list()
            .into_iter()
            .find(|p| p.id == "navidrome:pl-1")
            .unwrap();
        assert_eq!(changed.track_count, 3);
        assert_eq!(changed.updated_at, 1_769_904_000_123);

        // A rename without a new `changed` value is picked up from the listing alone.
        env.server
            .edit(|lib| lib.playlists[0].summary.name = "Renamed".into());
        assert!(env.sync());
        assert_eq!(env.requests("getPlaylist"), 3);
        assert!(env.list().iter().any(|p| p.name == "Renamed"));
    }

    #[test]
    fn removed_playlist_disappears() {
        let env = env();
        env.sync();
        env.server.edit(|lib| {
            lib.playlists.remove(0);
        });
        assert!(env.sync());
        let listed = env.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "navidrome:pl-1");
        assert_eq!(
            env.count(
                "SELECT COUNT(*) FROM remote_playlist_tracks WHERE playlist_id = 'navidrome:pl-0'"
            ),
            0
        );
        assert!(entries(&env.db.0.lock().unwrap(), "navidrome:pl-0").is_err());
        // The server lists none: every playlist goes.
        env.server.edit(|lib| lib.playlists.clear());
        assert!(env.sync());
        assert!(env.list().is_empty());
        assert_eq!(env.count("SELECT COUNT(*) FROM remote_playlist_tracks"), 0);
    }

    #[test]
    fn entries_of_tracks_missing_from_index_are_kept() {
        let env = env();
        // Playlists before the library: no entry resolves yet.
        env.sync();
        let entries = env.entries("navidrome:pl-0");
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].track_id, "navidrome:tr-0");
        assert!(entries.iter().all(|e| e.track.is_none()));
        let listed = env.list();
        let first = listed.iter().find(|p| p.id == "navidrome:pl-0").unwrap();
        assert_eq!(first.track_count, 5);
        assert!(first.cover_ids.is_empty());
        // The tracks arrive: the same entries resolve, nothing refetched.
        env.library();
        assert!(env
            .entries("navidrome:pl-0")
            .iter()
            .all(|e| e.track.is_some()));
        assert!(!env.sync());
        // A track leaves the index: its entry stays, unresolved.
        env.db
            .0
            .lock()
            .unwrap()
            .execute("DELETE FROM tracks WHERE id = 'navidrome:tr-2'", [])
            .unwrap();
        let entries = env.entries("navidrome:pl-0");
        assert_eq!(entries.len(), 5);
        assert!(entries[2].track.is_none());
        assert_eq!(entries[2].track_id, "navidrome:tr-2");
        assert!(entries[3].track.is_some());
    }

    #[test]
    fn wipe_clears_remote_playlists() {
        let env = env();
        env.sync();
        assert_eq!(env.count("SELECT COUNT(*) FROM remote_playlists"), 2);
        ensure_server(&env.db, "first").unwrap();
        assert_eq!(env.count("SELECT COUNT(*) FROM remote_playlists"), 0);
        assert_eq!(env.count("SELECT COUNT(*) FROM remote_playlist_tracks"), 0);
        assert!(env.list().is_empty());
        // The next sync fetches every playlist again.
        assert!(env.sync());
        assert_eq!(env.requests("getPlaylist"), 4);
    }

    fn change_both(env: &Env, changed: &str) {
        let changed = changed.to_owned();
        env.server.edit(move |lib| {
            for playlist in &mut lib.playlists {
                playlist.summary.changed = Some(changed.clone());
                playlist.song_ids.push("tr-9".into());
            }
        });
    }

    fn changed_of(env: &Env, id: &str) -> Option<String> {
        env.db
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT changed FROM remote_playlists WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn failing_playlist_is_skipped_and_refetched() {
        let env = env();
        env.sync();
        change_both(&env, "2026-03-01T00:00:00Z");
        env.server.set_fault(Fault::CodeFor {
            method: "getPlaylist",
            id: "pl-1",
            code: 0,
        });
        // The other playlist syncs; the failing one keeps its rows and its old `changed`.
        assert!(env.sync());
        assert_eq!(env.entries("navidrome:pl-0").len(), 6);
        assert_eq!(
            ids(&env.entries("navidrome:pl-1")),
            vec!["navidrome:tr-0", "navidrome:tr-7"]
        );
        assert_eq!(
            changed_of(&env, "navidrome:pl-1").as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(env.list().len(), 2);
        // Still failing: still kept, still asked for.
        let before = env.requests("getPlaylist");
        assert!(!env.sync());
        assert_eq!(env.requests("getPlaylist"), before + 1);
        assert_eq!(env.entries("navidrome:pl-1").len(), 2);
        // It answers again: fetched next cycle.
        env.server.set_fault(Fault::None);
        assert!(env.sync());
        assert_eq!(
            ids(&env.entries("navidrome:pl-1")),
            vec!["navidrome:tr-0", "navidrome:tr-7", "navidrome:tr-9"]
        );
        assert_eq!(
            changed_of(&env, "navidrome:pl-1").as_deref(),
            Some("2026-03-01T00:00:00Z")
        );
    }

    #[test]
    fn failing_new_playlist_is_left_out_until_it_answers() {
        let env = env();
        env.server.set_fault(Fault::CodeFor {
            method: "getPlaylist",
            id: "pl-0",
            code: 0,
        });
        assert!(env.sync());
        let listed = env.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "navidrome:pl-1");
        env.server.set_fault(Fault::None);
        assert!(env.sync());
        assert_eq!(env.list().len(), 2);
        assert_eq!(env.entries("navidrome:pl-0").len(), 5);
    }

    #[test]
    fn account_and_offline_failures_of_a_playlist_still_end_the_sync() {
        let env = env();
        env.sync();
        change_both(&env, "2026-03-01T00:00:00Z");
        env.server.set_fault(Fault::CodeFor {
            method: "getPlaylist",
            id: "pl-1",
            code: 40,
        });
        let result = sync_playlists(&env.db, &env.client);
        assert!(
            matches!(
                result,
                Err(SyncError::Api(NavidromeError::WrongCredentials))
            ),
            "{result:?}"
        );
        env.server.set_fault(Fault::CutOnce {
            method: "getPlaylist",
            after_bytes: 0,
        });
        let result = sync_playlists(&env.db, &env.client);
        assert!(
            matches!(&result, Err(SyncError::Api(err)) if err.is_offline()),
            "{result:?}"
        );
        // Neither wrote anything: both playlists are as before.
        assert_eq!(env.entries("navidrome:pl-0").len(), 5);
        assert_eq!(env.entries("navidrome:pl-1").len(), 2);
    }

    #[test]
    fn summary_scan_follows_the_primary_key() {
        let env = env();
        assert_scan_needs_no_sort(&env.db.0.lock().unwrap(), "remote_playlist_tracks");
    }

    #[test]
    fn cancelled_sync_writes_nothing() {
        let env = env();
        let result = sync_playlists_cancellable(&env.db, &env.client, 1, &|| true);
        assert!(matches!(result, Err(SyncError::Cancelled)));
        assert_eq!(env.count("SELECT COUNT(*) FROM remote_playlists"), 0);
    }

    #[test]
    fn prism_playlist_holds_navidrome_tracks() {
        let env = env();
        env.library();
        let id = {
            let conn = env.db.0.lock().unwrap();
            let playlist = playlist_create(&conn, "Mixed").unwrap();
            playlist_add(
                &conn,
                playlist.id,
                &["navidrome:tr-1".to_string(), "navidrome:tr-3".to_string()],
            )
            .unwrap();
            playlist.id
        };
        {
            let conn = env.db.0.lock().unwrap();
            let entries = playlist_entries(&conn, id).unwrap();
            assert!(entries.iter().all(|e| e.track.is_some()));
            let summary = playlists_list(&conn).unwrap().remove(0);
            assert_eq!(summary.track_count, 2);
            assert_eq!(summary.cover_ids, vec!["nd-al-0_cover".to_string()]);
        }
        // Another server: the Navidrome rows go, the user's entries stay.
        ensure_server(&env.db, "another").unwrap();
        let conn = env.db.0.lock().unwrap();
        let entries = playlist_entries(&conn, id).unwrap();
        assert_eq!(ids(&entries), vec!["navidrome:tr-1", "navidrome:tr-3"]);
        assert!(entries.iter().all(|e| e.track.is_none()));
        let summary = playlists_list(&conn).unwrap().remove(0);
        assert_eq!(summary.track_count, 2);
        assert_eq!(summary.duration_ms, 0);
        assert!(summary.cover_ids.is_empty());
    }
}
