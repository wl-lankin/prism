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
const MIGRATIONS: &[Migration] = &[
    Migration::Sql(include_str!("migrations/0001_init.sql")),
    Migration::Sql(include_str!("migrations/0002_indexes.sql")),
];

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

fn run_migrations(conn: &mut Connection, steps: &[Migration]) -> anyhow::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current < 0 || current as usize > steps.len() {
        anyhow::bail!(
            "library database schema version {current} is not supported (this build knows up to {})",
            steps.len()
        );
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
