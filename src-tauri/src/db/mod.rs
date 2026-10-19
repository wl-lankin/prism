use std::path::Path;
use std::sync::Mutex;

use anyhow::Context;
use rusqlite::Connection;

/// Ordered migrations; index + 1 is the `PRAGMA user_version` they produce.
const MIGRATIONS: &[&str] = &[include_str!("migrations/0001_init.sql")];

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
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
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

    #[test]
    fn migrates_fresh_db_to_latest() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(user_version(&db), 1);
    }

    #[test]
    fn migration_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("prism-db-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.db");
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(user_version(&db), 1);
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(user_version(&db), 1);
        let mode: String = db
            .0
            .lock()
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
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
                "SELECT t.title FROM tracks_fts f JOIN tracks t ON t.rowid = f.rowid WHERE tracks_fts MATCH 'tear*'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found, "Teardrop");
    }
}
