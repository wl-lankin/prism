pub mod rekey;

use std::path::Path;
use std::sync::Mutex;

use anyhow::Context;
use rusqlite::Connection;

/// One schema step. SQL steps are batches; Rust steps handle data rewrites SQL cannot express.
pub enum Migration {
    Sql(&'static str),
    Rust(fn(&rusqlite::Transaction) -> anyhow::Result<()>),
}

/// Ordered migrations; index + 1 is the `PRAGMA user_version` they produce.
pub const MIGRATIONS: &[Migration] = &[
    Migration::Sql(include_str!("migrations/0001_init.sql")),
    Migration::Sql(include_str!("migrations/0002_indexes.sql")),
    Migration::Rust(rekey::migrate_0003),
    Migration::Sql(include_str!("migrations/0004_favorites.sql")),
    Migration::Sql(include_str!("migrations/0005_track_stats.sql")),
    Migration::Sql(include_str!("migrations/0006_playlists.sql")),
    Migration::Sql(include_str!("migrations/0007_cover_lookups.sql")),
];

/// The database was written by a newer build than this one. Never quarantined: the file is fine.
#[derive(Debug)]
pub struct NewerSchema {
    pub found: i64,
    pub known: usize,
}

impl std::fmt::Display for NewerSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "library database schema version {} is not supported (this build knows up to {})",
            self.found, self.known
        )
    }
}

impl std::error::Error for NewerSchema {}

/// Track upserts must use `INSERT ... ON CONFLICT(id) DO UPDATE`. `INSERT OR REPLACE`
/// deletes the old row without firing the FTS delete trigger (recursive_triggers is off)
/// and would leave stale `tracks_fts` entries.
pub struct Db(pub Mutex<Connection>);

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Db> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening library database {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Db> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> anyhow::Result<Db> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut conn)?;
        Ok(Db(Mutex::new(conn)))
    }
}

fn migrate(conn: &mut Connection) -> anyhow::Result<()> {
    run_migrations(conn, MIGRATIONS)
}

pub(crate) fn run_migrations(conn: &mut Connection, steps: &[Migration]) -> anyhow::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current > steps.len() as i64 {
        return Err(NewerSchema {
            found: current,
            known: steps.len(),
        }
        .into());
    }
    if current < 0 {
        anyhow::bail!("library database schema version {current} is not valid");
    }
    for (index, step) in steps.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        match step {
            Migration::Sql(sql) => tx.execute_batch(sql).map_err(anyhow::Error::from),
            Migration::Rust(run) => run(&tx),
        }
        .with_context(|| format!("running migration {}", index + 1))?;
        tx.pragma_update(None, "user_version", (index + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_version(db: &Db) -> i64 {
        let conn = db.0.lock().unwrap();
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    use crate::library::query::{ALBUM_ORDER, TRACK_ORDER};

    #[test]
    fn migrates_fresh_db_to_latest() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(user_version(&db), MIGRATIONS.len() as i64);
    }

    fn rust_insert(tx: &rusqlite::Transaction) -> anyhow::Result<()> {
        tx.execute("INSERT INTO marker (n) VALUES (1)", [])?;
        Ok(())
    }

    #[test]
    fn rust_step_runs_once() {
        let steps = [
            Migration::Sql("CREATE TABLE marker (n INTEGER NOT NULL);"),
            Migration::Rust(rust_insert),
        ];
        let dir = std::env::temp_dir().join(format!("prism-db-rust-step-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("steps.db");
        for _ in 0..2 {
            let mut conn = Connection::open(&path).unwrap();
            run_migrations(&mut conn, &steps).unwrap();
        }
        let conn = Connection::open(&path).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM marker", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_negative_user_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", -1_i64).unwrap();
        assert!(migrate(&mut conn).is_err());
    }

    fn plan(db: &Db, sql: &str, params: &[&dyn rusqlite::ToSql]) -> String {
        let conn = db.0.lock().unwrap();
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = stmt
            .query_map(params, |row| row.get::<_, String>(3))
            .unwrap();
        rows.map(|r| r.unwrap()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn tracks_range_uses_order_index() {
        let db = Db::open_in_memory().unwrap();
        let plan = plan(
            &db,
            &format!("SELECT id FROM tracks t ORDER BY {TRACK_ORDER} LIMIT 200 OFFSET 0"),
            &[],
        );
        assert!(plan.contains("idx_tracks_order"), "{plan}");
        assert!(!plan.contains("USE TEMP B-TREE"), "{plan}");
    }

    #[test]
    fn albums_range_uses_order_index() {
        let db = Db::open_in_memory().unwrap();
        let plan = plan(
            &db,
            &format!("SELECT id FROM albums a ORDER BY {ALBUM_ORDER} LIMIT 200 OFFSET 0"),
            &[],
        );
        assert!(plan.contains("idx_albums_order"), "{plan}");
        assert!(!plan.contains("USE TEMP B-TREE"), "{plan}");
    }

    #[test]
    fn folder_lookup_uses_index() {
        let db = Db::open_in_memory().unwrap();
        let plan = plan(
            &db,
            "SELECT id FROM tracks WHERE folder = ?1",
            &[&"C:\\Music"],
        );
        assert!(plan.contains("idx_tracks_folder"), "{plan}");
    }

    #[test]
    fn migration_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("prism-db-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.db");
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(user_version(&db), MIGRATIONS.len() as i64);
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(user_version(&db), MIGRATIONS.len() as i64);
        let mode: String =
            db.0.lock()
                .unwrap()
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .unwrap();
        assert_eq!(mode, "wal");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_schema_from_newer_build() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 99_i64).unwrap();
        assert!(migrate(&mut conn).is_err());
    }

    #[test]
    fn fts_follows_update_and_delete() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        let upsert = |title: &str| {
            conn.execute(
                "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id, duration_ms, format)
                 VALUES ('t1', 'local', ?1, 'Massive Attack', 'Massive Attack', 'Mezzanine', 'a1', 1000, 'flac')
                 ON CONFLICT(id) DO UPDATE SET title = excluded.title",
                [title],
            )
            .unwrap();
        };
        let hits = |query: &str| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH ?1",
                [query],
                |row| row.get(0),
            )
            .unwrap()
        };
        upsert("Teardrop");
        assert_eq!(hits("tear*"), 1);
        upsert("Angel");
        assert_eq!(hits("tear*"), 0);
        assert_eq!(hits("angel*"), 1);
        conn.execute("DELETE FROM tracks WHERE id = 't1'", [])
            .unwrap();
        assert_eq!(hits("angel*"), 0);
    }

    #[test]
    fn upgrades_m1_db_with_data() {
        use sha1::{Digest, Sha1};

        use crate::library::model::{album_id, track_id};

        let mut conn = Connection::open_in_memory().unwrap();
        run_migrations(&mut conn, &MIGRATIONS[..1]).unwrap();
        let folder = std::env::temp_dir().join("prism-m1-music");
        let folder_key = folder.to_string_lossy().into_owned();
        let songs = [
            ("Massive Attack", "Mezzanine", "Teardrop"),
            ("Massive Attack", "Mezzanine", "Angel"),
            ("Portishead", "Dummy", "Roads"),
        ];
        let mut expected = Vec::new();
        for (artist, album, title) in songs {
            let rel = Path::new(artist).join(album).join(format!("{title}.flac"));
            let path = folder.join(&rel).to_string_lossy().into_owned();
            // M1 ids: "local:" + sha1 of the canonical path.
            let old_id = format!("local:{}", hex::encode(Sha1::digest(path.as_bytes())));
            let album_key = album_id(artist, album);
            conn.execute(
                "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                    duration_ms, format, path, folder, mtime, size)
                 VALUES (?1, 'local', ?2, ?3, ?3, ?4, ?5, 1000, 'flac', ?6, ?7, 1, 1)",
                rusqlite::params![old_id, title, artist, album, album_key, path, folder_key],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO albums (id, source, title, artist, track_count, duration_ms)
                 VALUES (?1, 'local', ?2, ?3, 1, 1000)
                 ON CONFLICT(id) DO UPDATE SET track_count = track_count + 1",
                rusqlite::params![album_key, album, artist],
            )
            .unwrap();
            expected.push((
                title.to_string(),
                track_id(&folder_key, &rel),
                album_key,
                old_id,
            ));
        }
        conn.execute(
            "INSERT INTO folders (path, online) VALUES (?1, 1)",
            [&folder_key],
        )
        .unwrap();
        let albums_before: Vec<(String, i64)> = {
            let mut stmt = conn
                .prepare("SELECT id, track_count FROM albums ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap();
            rows.map(|row| row.unwrap()).collect()
        };

        run_migrations(&mut conn, MIGRATIONS).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, songs.len() as i64);
        for (title, new_id, album_key, old_id) in &expected {
            let (id, album): (String, String) = conn
                .query_row(
                    "SELECT id, album_id FROM tracks WHERE title = ?1",
                    [title],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(&id, new_id, "{title}");
            assert_ne!(&id, old_id, "{title}");
            assert_eq!(&album, album_key, "{title}");
        }
        let albums_after: Vec<(String, i64)> = {
            let mut stmt = conn
                .prepare("SELECT id, track_count FROM albums ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap();
            rows.map(|row| row.unwrap()).collect()
        };
        assert_eq!(albums_after, albums_before);
        let found: String = conn
            .query_row(
                "SELECT t.id FROM tracks_fts f JOIN tracks t ON t.row_id = f.rowid
                 WHERE tracks_fts MATCH 'roads'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found, expected[2].1);
        let massive: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH 'massive'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(massive, 2);
    }

    #[test]
    fn fts_finds_inserted_track() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id, duration_ms, format)
             VALUES ('t1', 'local', 'Teardrop', 'Massive Attack', 'Massive Attack', 'Mezzanine', 'a1', 1000, 'flac')",
            [],
        )
        .unwrap();
        let found: String = conn
            .query_row(
                "SELECT t.title FROM tracks_fts f JOIN tracks t ON t.row_id = f.rowid WHERE tracks_fts MATCH 'tear*'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found, "Teardrop");
    }
}
