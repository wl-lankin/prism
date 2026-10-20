//! Track id rewrites: migration 3 moves M1 ids (sha1 of the canonical path) to ids from the
//! folder key and the path relative to it.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{params, OptionalExtension, Transaction};

use crate::library::model::track_id;
use crate::library::scan::rebuild_albums;

/// A column that stores track ids and follows them when tracks are rekeyed. `filter` is an
/// SQL condition on the same table that limits which rows hold track ids (for tables that
/// mix kinds of ids). The column must not carry a foreign key to `tracks(id)`: rows that
/// lose a collision are deleted before their refs move to the winner. A column with a
/// unique constraint must cope with two old ids becoming one (a merge).
pub struct TrackIdRef {
    pub table: &'static str,
    pub column: &'static str,
    pub filter: Option<&'static str>,
}

/// Every column outside `tracks` that stores a track id.
pub const TRACK_ID_REFS: &[TrackIdRef] = &[];

/// How many rows that keep their id `migrate_0003` names individually in the log.
const LOGGED_KEPT_ROWS: usize = 20;

/// Renames tracks from the first id of each pair to the second, in the caller's
/// transaction, and rewrites every registered ref whose table exists. When several rows end
/// up with one id (several old ids map to it, or a row already holds it), the row with the
/// lowest `row_id` wins and keeps its data; the others are deleted and their refs point to
/// the winner. FTS rows follow through the update and delete triggers (`row_id` is kept).
pub fn rekey_tracks(tx: &Transaction, map: &[(String, String)]) -> anyhow::Result<()> {
    rekey_tracks_with(tx, map, TRACK_ID_REFS)
}

/// A track row taking part in a rename: its `row_id`, current id and album.
struct Member<'a> {
    row_id: i64,
    id: &'a str,
    album_id: String,
}

fn rekey_tracks_with(
    tx: &Transaction,
    map: &[(String, String)],
    refs: &[TrackIdRef],
) -> anyhow::Result<()> {
    let mut targets: HashMap<&str, &str> = HashMap::with_capacity(map.len());
    for (old, new) in map {
        if let Some(previous) = targets.insert(old.as_str(), new.as_str()) {
            if previous != new {
                anyhow::bail!("track {old} is mapped to both {previous} and {new}");
            }
        }
    }
    if targets.is_empty() {
        return Ok(());
    }

    // Group the existing rows by their new id, in the order of `map`.
    let mut find = tx.prepare("SELECT row_id, album_id FROM tracks WHERE id = ?1")?;
    let mut lookup = |id: &str| -> rusqlite::Result<Option<(i64, String)>> {
        find.query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
    };
    let mut order: Vec<&str> = Vec::new();
    let mut groups: HashMap<&str, Vec<Member>> = HashMap::new();
    let mut listed: HashSet<&str> = HashSet::with_capacity(targets.len());
    for (old, new) in map {
        if !listed.insert(old.as_str()) {
            continue;
        }
        if let Some((row_id, album_id)) = lookup(old)? {
            let members = groups.entry(new.as_str()).or_insert_with(|| {
                order.push(new.as_str());
                Vec::new()
            });
            members.push(Member {
                row_id,
                id: old.as_str(),
                album_id,
            });
        }
    }
    // A row that already holds a new id and is not renamed itself competes for that id.
    for new in &order {
        if targets.contains_key(new) {
            continue;
        }
        if let Some((row_id, album_id)) = lookup(new)? {
            groups.get_mut(new).expect("grouped").push(Member {
                row_id,
                id: new,
                album_id,
            });
        }
    }

    let mut delete = tx.prepare("DELETE FROM tracks WHERE row_id = ?1")?;
    let mut touched_albums: HashSet<String> = HashSet::new();
    let mut renames: Vec<(i64, &str)> = Vec::new();
    for new in &order {
        let members = groups.get_mut(new).expect("grouped");
        members.sort_by_key(|member| member.row_id);
        for loser in &members[1..] {
            eprintln!(
                "track {} is the same file as an earlier track; merged into {new}",
                loser.id
            );
            delete.execute([loser.row_id])?;
            touched_albums.insert(loser.album_id.clone());
        }
        let winner = &members[0];
        if winner.id != *new {
            renames.push((winner.row_id, new));
        }
    }

    rename_rows(tx, &renames)?;
    rewrite_refs(tx, &targets, refs)?;
    rebuild_albums(tx, touched_albums.iter())?;
    Ok(())
}

/// Gives each `(row_id, new id)` row its new id with one set-based UPDATE (about twice as
/// fast as one UPDATE per row on a large library). Every new id is unique among the pairs,
/// so the only possible UNIQUE conflict is a new id still held by another renamed row (a
/// chain or a swap). Then all renamed rows first move to temporary ids (ids never start
/// with U+001F), which frees every target before the final UPDATE.
fn rename_rows(tx: &Transaction, renames: &[(i64, &str)]) -> anyhow::Result<()> {
    if renames.is_empty() {
        return Ok(());
    }
    tx.execute_batch(
        "DROP TABLE IF EXISTS temp.prism_rekey_rows;
         CREATE TEMP TABLE prism_rekey_rows (row_id INTEGER PRIMARY KEY, new TEXT NOT NULL);",
    )?;
    {
        let mut insert =
            tx.prepare("INSERT INTO temp.prism_rekey_rows (row_id, new) VALUES (?1, ?2)")?;
        for (row_id, new) in renames {
            insert.execute(params![row_id, new])?;
        }
    }
    let held: i64 = tx.query_row(
        "SELECT COUNT(*) FROM temp.prism_rekey_rows r JOIN tracks t ON t.id = r.new",
        [],
        |row| row.get(0),
    )?;
    if held > 0 {
        tx.execute(
            "UPDATE tracks SET id = char(31) || 'rekey:' || row_id
             WHERE row_id IN (SELECT row_id FROM temp.prism_rekey_rows)",
            [],
        )?;
    }
    tx.execute(
        "UPDATE tracks SET id = r.new FROM temp.prism_rekey_rows AS r
         WHERE tracks.row_id = r.row_id",
        [],
    )?;
    tx.execute_batch("DROP TABLE temp.prism_rekey_rows;")?;
    Ok(())
}

/// Rewrites every ref column whose table exists, all pairs at once (through a temp table),
/// so chains and swaps of ids cannot rewrite a value twice.
fn rewrite_refs(
    tx: &Transaction,
    targets: &HashMap<&str, &str>,
    refs: &[TrackIdRef],
) -> anyhow::Result<()> {
    let mut exists =
        tx.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?;
    let mut present: Vec<&TrackIdRef> = Vec::new();
    for reference in refs {
        if exists.exists([reference.table])? {
            present.push(reference);
        }
    }
    if present.is_empty() {
        return Ok(());
    }
    tx.execute_batch(
        "DROP TABLE IF EXISTS temp.prism_rekey_map;
         CREATE TEMP TABLE prism_rekey_map (old TEXT PRIMARY KEY, new TEXT NOT NULL);",
    )?;
    {
        let mut insert =
            tx.prepare("INSERT INTO temp.prism_rekey_map (old, new) VALUES (?1, ?2)")?;
        for (old, new) in targets {
            if old != new {
                insert.execute([old, new])?;
            }
        }
    }
    for reference in present {
        let (table, column) = (reference.table, reference.column);
        let filter = reference
            .filter
            .map(|condition| format!(" AND ({condition})"))
            .unwrap_or_default();
        tx.execute(
            &format!(
                "UPDATE \"{table}\" SET \"{column}\" =
                    (SELECT new FROM temp.prism_rekey_map WHERE old = \"{table}\".\"{column}\")
                 WHERE \"{column}\" IN (SELECT old FROM temp.prism_rekey_map){filter}"
            ),
            [],
        )?;
    }
    tx.execute_batch("DROP TABLE temp.prism_rekey_map;")?;
    Ok(())
}

/// The id a local track gets from its folder and path, or `None` when the path does not lie
/// below the folder (or either is missing).
fn new_id(folder: Option<&str>, path: Option<&str>) -> Option<String> {
    let folder = folder?;
    let relative = Path::new(path?).strip_prefix(Path::new(folder)).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    Some(track_id(folder, relative))
}

/// Migration 3: every local track gets its id from `folder` and the path relative to it.
/// Rows whose path is not below their folder keep their id and are logged.
pub fn migrate_0003(tx: &Transaction) -> anyhow::Result<()> {
    let mut map: Vec<(String, String)> = Vec::new();
    let mut kept = 0usize;
    {
        let mut stmt = tx.prepare(
            "SELECT id, folder, path FROM tracks WHERE source = 'local' ORDER BY row_id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let folder: Option<String> = row.get(1)?;
            let path: Option<String> = row.get(2)?;
            match new_id(folder.as_deref(), path.as_deref()) {
                Some(new) if new == id => {}
                Some(new) => map.push((id, new)),
                None => {
                    kept += 1;
                    if kept <= LOGGED_KEPT_ROWS {
                        eprintln!(
                            "track {id} keeps its id: its path {} is not inside its folder {}",
                            path.as_deref().unwrap_or("(none)"),
                            folder.as_deref().unwrap_or("(none)")
                        );
                    }
                }
            }
        }
    }
    if kept > LOGGED_KEPT_ROWS {
        eprintln!(
            "{kept} tracks in all keep their id because their path is not inside their folder"
        );
    }
    rekey_tracks(tx, &map)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use rusqlite::{params, Connection};

    use super::*;
    use crate::db::Db;
    use crate::library::model::track_id;

    /// A library folder spelled like the OS spells absolute paths. Nothing is created on disk.
    fn music() -> PathBuf {
        std::env::temp_dir().join("prism-rekey-music")
    }

    fn key(folder: &Path) -> String {
        folder.to_string_lossy().into_owned()
    }

    fn insert(conn: &Connection, id: &str, folder: &Path, path: &str, title: &str) {
        conn.execute(
            "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                duration_ms, format, path, folder, mtime, size)
             VALUES (?1, 'local', ?2, 'Massive Attack', 'Massive Attack', 'Mezzanine', 'local:album',
                1000, 'flac', ?3, ?4, 1, 1)",
            params![id, title, path, key(folder)],
        )
        .unwrap();
    }

    fn rebuild_album(conn: &Connection) {
        crate::library::scan::rebuild_albums(conn, ["local:album".to_string()].iter()).unwrap();
    }

    fn ids(conn: &Connection) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare("SELECT title, id FROM tracks ORDER BY row_id")
            .unwrap();
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        rows.map(|row| row.unwrap()).collect()
    }

    fn hits(conn: &Connection, query: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH ?1",
            [query],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn migrate(conn: &mut Connection) {
        let tx = conn.transaction().unwrap();
        migrate_0003(&tx).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn rekey_rewrites_ids_and_keeps_fts() {
        let db = Db::open_in_memory().unwrap();
        let mut conn = db.0.lock().unwrap();
        let folder = music();
        let teardrop = Path::new("Massive Attack").join("Teardrop.flac");
        let angel = Path::new("Massive Attack").join("Angel.flac");
        insert(
            &conn,
            "local:old-1",
            &folder,
            &key(&folder.join(&teardrop)),
            "Teardrop",
        );
        insert(
            &conn,
            "local:old-2",
            &folder,
            &key(&folder.join(&angel)),
            "Angel",
        );
        migrate(&mut conn);
        assert_eq!(
            ids(&conn),
            vec![
                ("Teardrop".to_string(), track_id(&key(&folder), &teardrop)),
                ("Angel".to_string(), track_id(&key(&folder), &angel)),
            ]
        );
        assert_eq!(hits(&conn, "teardrop"), 1);
        assert_eq!(hits(&conn, "angel"), 1);
        assert_eq!(hits(&conn, "massive"), 2);
        let found: String = conn
            .query_row(
                "SELECT t.id FROM tracks_fts f JOIN tracks t ON t.row_id = f.rowid
                 WHERE tracks_fts MATCH 'tear*'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found, track_id(&key(&folder), &teardrop));
    }

    #[test]
    fn rekey_merges_collisions() {
        let db = Db::open_in_memory().unwrap();
        let mut conn = db.0.lock().unwrap();
        let folder = music();
        let sep = std::path::MAIN_SEPARATOR;
        let clean = key(&folder.join("sub").join("a.flac"));
        // The same file spelled twice (a doubled separator): both rows map to one new id.
        let doubled = format!("{}{sep}sub{sep}{sep}a.flac", key(&folder));
        insert(&conn, "local:old-first", &folder, &clean, "First");
        insert(&conn, "local:old-second", &folder, &doubled, "Second");
        rebuild_album(&conn);
        assert_eq!(count(&conn, "SELECT track_count FROM albums"), 2);
        migrate(&mut conn);
        let rel = Path::new("sub").join("a.flac");
        assert_eq!(
            ids(&conn),
            vec![("First".to_string(), track_id(&key(&folder), &rel))]
        );
        assert_eq!(hits(&conn, "second"), 0);
        assert_eq!(hits(&conn, "first"), 1);
        assert_eq!(count(&conn, "SELECT track_count FROM albums"), 1);
    }

    #[test]
    fn rekey_keeps_rows_outside_their_folder() {
        let db = Db::open_in_memory().unwrap();
        let mut conn = db.0.lock().unwrap();
        let folder = music();
        let elsewhere = std::env::temp_dir()
            .join("prism-rekey-elsewhere")
            .join("x.flac");
        insert(
            &conn,
            "local:old-outside",
            &folder,
            &key(&elsewhere),
            "Outside",
        );
        conn.execute(
            "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                duration_ms, format)
             VALUES ('local:no-path', 'local', 'Pathless', 'A', 'A', 'B', 'local:album', 1, 'flac')",
            [],
        )
        .unwrap();
        migrate(&mut conn);
        assert_eq!(
            ids(&conn),
            vec![
                ("Outside".to_string(), "local:old-outside".to_string()),
                ("Pathless".to_string(), "local:no-path".to_string()),
            ]
        );
    }

    #[test]
    fn rekey_handles_swapped_ids() {
        let db = Db::open_in_memory().unwrap();
        let conn = &mut *db.0.lock().unwrap();
        let folder = music();
        insert(conn, "a", &folder, &key(&folder.join("a.flac")), "Alpha");
        insert(conn, "b", &folder, &key(&folder.join("b.flac")), "Beta");
        let tx = conn.transaction().unwrap();
        let map = vec![
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "a".to_string()),
        ];
        rekey_tracks(&tx, &map).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            ids(conn),
            vec![
                ("Alpha".to_string(), "b".to_string()),
                ("Beta".to_string(), "a".to_string()),
            ]
        );
        assert_eq!(hits(conn, "alpha"), 1);
    }

    #[test]
    fn rekey_updates_registered_refs() {
        let db = Db::open_in_memory().unwrap();
        let conn = &mut *db.0.lock().unwrap();
        let folder = music();
        insert(
            conn,
            "old-1",
            &folder,
            &key(&folder.join("a.flac")),
            "Alpha",
        );
        insert(conn, "old-2", &folder, &key(&folder.join("b.flac")), "Beta");
        conn.execute_batch(
            "CREATE TABLE test_refs (n INTEGER PRIMARY KEY, kind TEXT NOT NULL, item TEXT NOT NULL);
             INSERT INTO test_refs (kind, item) VALUES
                ('track', 'old-1'), ('album', 'old-1'), ('track', 'old-2'), ('track', 'other');",
        )
        .unwrap();
        let refs = [
            TrackIdRef {
                table: "test_refs",
                column: "item",
                filter: Some("kind = 'track'"),
            },
            TrackIdRef {
                table: "test_missing_table",
                column: "item",
                filter: None,
            },
        ];
        let tx = conn.transaction().unwrap();
        let map = vec![
            ("old-1".to_string(), "new-1".to_string()),
            ("old-2".to_string(), "new-2".to_string()),
        ];
        rekey_tracks_with(&tx, &map, &refs).unwrap();
        tx.commit().unwrap();
        let mut stmt = conn
            .prepare("SELECT kind, item FROM test_refs ORDER BY n")
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            rows,
            vec![
                pair("track", "new-1"),
                pair("album", "old-1"),
                pair("track", "new-2"),
                pair("track", "other"),
            ]
        );
        assert_eq!(
            ids(conn),
            vec![pair("Alpha", "new-1"), pair("Beta", "new-2")]
        );
    }

    #[test]
    fn rekey_merge_rewrites_refs_to_the_winner() {
        let db = Db::open_in_memory().unwrap();
        let conn = &mut *db.0.lock().unwrap();
        let folder = music();
        insert(
            conn,
            "old-1",
            &folder,
            &key(&folder.join("a.flac")),
            "Alpha",
        );
        insert(conn, "old-2", &folder, &key(&folder.join("b.flac")), "Beta");
        conn.execute_batch(
            "CREATE TABLE test_refs (n INTEGER PRIMARY KEY, item TEXT NOT NULL);
             INSERT INTO test_refs (item) VALUES ('old-2'), ('old-1');",
        )
        .unwrap();
        let refs = [TrackIdRef {
            table: "test_refs",
            column: "item",
            filter: None,
        }];
        let tx = conn.transaction().unwrap();
        let map = vec![
            ("old-2".to_string(), "new".to_string()),
            ("old-1".to_string(), "new".to_string()),
        ];
        rekey_tracks_with(&tx, &map, &refs).unwrap();
        tx.commit().unwrap();
        assert_eq!(ids(conn), vec![("Alpha".to_string(), "new".to_string())]);
        assert_eq!(
            count(conn, "SELECT COUNT(*) FROM test_refs WHERE item = 'new'"),
            2
        );
    }

    /// 30k rows in one transaction on a WAL database file, the size of a large real library.
    /// Timing is only meaningful in an optimized build: `cargo test --release
    /// --manifest-path src-tauri/Cargo.toml rekey_30k -- --nocapture`.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timing; run with --release")]
    fn rekey_30k_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("library.db")).unwrap();
        let mut conn = db.0.lock().unwrap();
        let folder = music();
        {
            let tx = conn.transaction().unwrap();
            {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO tracks (id, source, title, artist, album_artist, album,
                            album_id, duration_ms, format, path, folder, mtime, size)
                         VALUES (?1, 'local', ?2, ?3, ?3, ?4, ?5, 1000, 'flac', ?6, ?7, 1, 1)",
                    )
                    .unwrap();
                for i in 0..30_000 {
                    let artist = format!("Artist {}", i % 500);
                    let album = format!("Album {}", i % 3000);
                    let path = folder
                        .join(&artist)
                        .join(&album)
                        .join(format!("{i:05} Track.flac"));
                    stmt.execute(params![
                        format!("local:old-{i}"),
                        format!("Track {i}"),
                        artist,
                        album,
                        format!("local:album-{}", i % 3000),
                        key(&path),
                        key(&folder),
                    ])
                    .unwrap();
                }
            }
            tx.commit().unwrap();
        }
        let started = std::time::Instant::now();
        migrate(&mut conn);
        let elapsed = started.elapsed();
        println!("rekey of 30000 rows took {elapsed:?}");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM tracks WHERE id LIKE 'local:old-%'"
            ),
            0
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM tracks"), 30_000);
        assert_eq!(hits(&conn, "\"track 12345\""), 1);
        // Typically 1 to 2 s; the bound only catches a pathological (per-row quadratic)
        // regression, with headroom for a loaded machine.
        assert!(elapsed.as_secs() < 15, "rekey took {elapsed:?}");
    }
}
