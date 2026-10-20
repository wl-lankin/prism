//! Startup: open the library database (quarantining a damaged one, backing up before a
//! migration) and report failures that happen before the web view exists.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use anyhow::Context;
use rusqlite::{Connection, ErrorCode};

use crate::db::{Db, NewerSchema, MIGRATIONS};

pub enum OpenOutcome {
    Opened(Db),
    Quarantined { db: Db, bad: PathBuf },
}

/// How many `library.db.pre-v<from>-v<to>` backups are kept.
const KEPT_BACKUPS: usize = 3;

/// What a look at the file on disk found.
enum Probe {
    /// Readable; carries its `user_version` (0 for a missing or empty file).
    Healthy(i64),
    Corrupt,
}

fn is_corrupt(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    )
}

/// Reads the schema version. A file SQLite cannot read at all fails here; subtler damage
/// surfaces later, while opening, backing up or migrating (see `is_corrupt_chain`).
fn probe(path: &Path) -> anyhow::Result<Probe> {
    if !path.exists() {
        return Ok(Probe::Healthy(0));
    }
    let read = Connection::open(path)
        .and_then(|conn| conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)));
    match read {
        Ok(version) => Ok(Probe::Healthy(version)),
        Err(err) if is_corrupt(&err) => Ok(Probe::Corrupt),
        Err(err) => Err(err).context("reading the library database"),
    }
}

fn is_corrupt_chain(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|cause| cause.downcast_ref::<rusqlite::Error>())
        .any(is_corrupt)
}

/// `library.db` plus a suffix, e.g. `library.db-wal` or `library.db.bad.2`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Opens the library database. A damaged file is moved aside (with its `-wal` and `-shm`)
/// and replaced by a fresh one; a database from a newer build is an error and stays put.
/// An existing database that still has migrations to run is backed up first.
pub fn open_or_quarantine(path: &Path) -> anyhow::Result<OpenOutcome> {
    let damaged = match probe(path)? {
        Probe::Corrupt => true,
        Probe::Healthy(version) => match open_checked(path, version) {
            Ok(db) => return Ok(OpenOutcome::Opened(db)),
            Err(err) if is_corrupt_chain(&err) => true,
            Err(err) => return Err(err),
        },
    };
    debug_assert!(damaged);
    let bad = quarantine(path)?;
    let db = Db::open(path)?;
    Ok(OpenOutcome::Quarantined { db, bad })
}

fn open_checked(path: &Path, version: i64) -> anyhow::Result<Db> {
    let latest = MIGRATIONS.len() as i64;
    // Checked here, before `Db::open` switches the file to WAL and so rewrites its header.
    if version > latest {
        return Err(NewerSchema {
            found: version,
            known: MIGRATIONS.len(),
        }
        .into());
    }
    if version > 0 && version < latest {
        backup_before_migration(path, version, latest)
            .context("backing up the library database before migrating it")?;
    }
    Db::open(path)
}

fn quarantine(path: &Path) -> anyhow::Result<PathBuf> {
    let mut bad = with_suffix(path, ".bad");
    let mut n = 2;
    while bad.exists() {
        bad = with_suffix(path, &format!(".bad.{n}"));
        n += 1;
    }
    for suffix in ["", "-wal", "-shm"] {
        let from = with_suffix(path, suffix);
        if from.exists() {
            let to = with_suffix(&bad, suffix);
            std::fs::rename(&from, &to)
                .with_context(|| format!("moving {} to {}", from.display(), to.display()))?;
        }
    }
    Ok(bad)
}

/// Copies the database to `library.db.pre-v<from>-v<to>` with `VACUUM INTO`, which reads
/// through SQLite and so includes whatever still sits in the WAL, then prunes old backups.
fn backup_before_migration(path: &Path, from: i64, to: i64) -> anyhow::Result<()> {
    let target = with_suffix(path, &format!(".pre-v{from}-v{to}"));
    let partial = with_suffix(&target, ".partial");
    for stale in [&target, &partial] {
        if stale.exists() {
            std::fs::remove_file(stale).with_context(|| format!("removing {}", stale.display()))?;
        }
    }
    let copy = || -> anyhow::Result<()> {
        let conn = Connection::open(path)?;
        conn.execute("VACUUM INTO ?1", [partial.to_string_lossy().as_ref()])?;
        drop(conn);
        std::fs::rename(&partial, &target)?;
        Ok(())
    };
    if let Err(err) = copy() {
        let _ = std::fs::remove_file(&partial);
        return Err(err).with_context(|| format!("writing {}", target.display()));
    }
    prune_backups(path);
    Ok(())
}

/// Keeps the newest `KEPT_BACKUPS` backups. Failing to delete an old one is harmless.
fn prune_backups(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let prefix = format!("{}.pre-v", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut found: Vec<(std::time::SystemTime, String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if !file_name.starts_with(&prefix) || file_name.ends_with(".partial") {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, file_name, entry.path()))
        })
        .collect();
    found.sort_by(|a, b| (b.0, &b.1).cmp(&(a.0, &a.1)));
    for (_, _, old) in found.into_iter().skip(KEPT_BACKUPS) {
        let _ = std::fs::remove_file(old);
    }
}

/// Data, config and cache folders.
pub struct AppDirs {
    pub data: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
}

/// The folders under a `PRISM_DATA_DIR` override, or `None` when it is unset or empty.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub fn dirs_under_override(value: Option<&std::ffi::OsStr>) -> Option<AppDirs> {
    let base = PathBuf::from(value.filter(|v| !v.is_empty())?);
    Some(AppDirs {
        data: base.join("data"),
        config: base.join("config"),
        cache: base.join("cache"),
    })
}

/// The data folder, attached as context to errors that happen while opening the database.
#[derive(Debug)]
pub struct DataDir(pub PathBuf);

impl std::fmt::Display for DataDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "data folder {}", self.0.display())
    }
}

/// The data folder could not be created.
#[derive(Debug)]
pub struct CannotCreateDataDir {
    pub path: PathBuf,
    pub source: std::io::Error,
}

impl std::fmt::Display for CannotCreateDataDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "creating {}: {}", self.path.display(), self.source)
    }
}

impl std::error::Error for CannotCreateDataDir {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// True when the system language is German. Only the startup dialog uses this: the web view
/// (and so the app's own language setting) is not available yet. Windows reads the user
/// locale, macOS the first preferred language, anything else `LC_ALL` / `LANG`.
fn system_is_german() -> bool {
    #[cfg(target_os = "windows")]
    {
        let mut buffer = [0_u16; 85];
        // SAFETY: the buffer is 85 u16, the documented LOCALE_NAME_MAX_LENGTH.
        let len = unsafe {
            windows_sys::Win32::Globalization::GetUserDefaultLocaleName(
                buffer.as_mut_ptr(),
                buffer.len() as i32,
            )
        };
        len > 0 && String::from_utf16_lossy(&buffer[..len as usize - 1]).starts_with("de")
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("defaults")
            .args(["read", "-g", "AppleLanguages"])
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
            .and_then(|text| text.split('"').nth(1).map(|lang| lang.starts_with("de")))
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        ["LC_ALL", "LANG"]
            .iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|value| !value.is_empty())
            .is_some_and(|value| value.starts_with("de"))
    }
}

/// Title and body of the startup dialog. The newer-schema and data-folder texts are English
/// and German; any other failure shows the error chain in English.
fn startup_message(err: &anyhow::Error, german: bool) -> (String, String) {
    let title = if german {
        "Prism kann nicht starten"
    } else {
        "Prism cannot start"
    };
    let body = if let Some(cause) = err.downcast_ref::<CannotCreateDataDir>() {
        let path = cause.path.display();
        if german {
            format!(
                "Prism kann seinen Datenordner nicht anlegen:\n{path}\n\nPrüfe, ob der Speicherort beschreibbar ist und ob keine Datei mit demselben Namen existiert."
            )
        } else {
            format!(
                "Prism cannot create its data folder:\n{path}\n\nCheck that the location is writable and that no file with the same name exists."
            )
        }
    } else if err.downcast_ref::<NewerSchema>().is_some() {
        let dir = err
            .downcast_ref::<DataDir>()
            .map(|dir| dir.0.display().to_string())
            .unwrap_or_default();
        if german {
            format!(
                "Eine neuere Version von Prism hat die Bibliotheksdatenbank in diesem Ordner geschrieben:\n{dir}\n\nDiese Version kann sie nicht lesen. Installiere die neuere Prism-Version oder verschiebe den Ordner, um mit einer leeren Mediathek zu starten."
            )
        } else {
            format!(
                "A newer version of Prism wrote the library database in this folder:\n{dir}\n\nThis version cannot read it. Install the newer version of Prism, or move the folder aside to start with an empty library."
            )
        }
    } else {
        format!("Prism could not start:\n{err:#}")
    };
    (title.to_string(), body)
}

/// Shows a native error dialog (synchronous, safe on the main thread during setup) and
/// exits with code 1. Used while the web view cannot show anything yet.
pub fn fail(title: &str, body: &str) -> ! {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(title)
        .set_description(body)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
    std::process::exit(1)
}

/// Reports a startup error in a dialog and exits.
pub fn fail_with(err: &anyhow::Error) -> ! {
    let (title, body) = startup_message(err, system_is_german());
    eprintln!("startup failed: {err:#}");
    fail(&title, &body)
}

/// A one-time message for the main window, e.g. the name of a quarantined library file.
pub struct StartupNotice(pub Mutex<Option<String>>);

impl StartupNotice {
    pub fn take(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// Returns the startup notice once and clears it.
#[tauri::command]
pub fn startup_notice(notice: tauri::State<'_, StartupNotice>) -> Option<String> {
    notice.take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{run_migrations, MIGRATIONS};
    use rusqlite::Connection;

    fn version_of(path: &Path) -> i64 {
        let conn = Connection::open(path).unwrap();
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    fn count_tracks(path: &Path) -> i64 {
        let conn = Connection::open(path).unwrap();
        conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))
            .unwrap()
    }

    fn user_version(db: &Db) -> i64 {
        db.0.lock()
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    fn v1_db(path: &Path) {
        let mut conn = Connection::open(path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        run_migrations(&mut conn, &MIGRATIONS[..1]).unwrap();
        for n in 0..3 {
            conn.execute(
                "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                    duration_ms, format)
                 VALUES (?1, 'local', 'T', 'A', 'A', 'B', 'a1', 1000, 'flac')",
                [format!("t{n}")],
            )
            .unwrap();
        }
    }

    fn backups(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("library.db.pre-v"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn dialog_names_the_folder_in_both_languages() {
        let dir = PathBuf::from("data-dir-x");
        let newer = anyhow::Error::new(NewerSchema {
            found: 99,
            known: 3,
        })
        .context(DataDir(dir.clone()));
        let cannot = anyhow::Error::new(CannotCreateDataDir {
            path: dir,
            source: std::io::Error::other("denied"),
        });
        for err in [&newer, &cannot] {
            let (en_title, en) = startup_message(err, false);
            let (de_title, de) = startup_message(err, true);
            assert_ne!(en_title, de_title);
            assert!(en.contains("data-dir-x") && de.contains("data-dir-x"));
            assert_ne!(en, de);
        }
        assert!(startup_message(&newer, false).1.contains("newer version"));
        assert!(startup_message(&cannot, false).1.contains("cannot create"));
        let other = anyhow::anyhow!("boom");
        assert!(startup_message(&other, false).1.contains("boom"));
    }

    #[test]
    fn override_relocates_all_dirs() {
        use std::ffi::OsStr;
        let dirs = dirs_under_override(Some(OsStr::new("base"))).unwrap();
        assert_eq!(dirs.data, PathBuf::from("base").join("data"));
        assert_eq!(dirs.config, PathBuf::from("base").join("config"));
        assert_eq!(dirs.cache, PathBuf::from("base").join("cache"));
        assert!(dirs_under_override(Some(OsStr::new(""))).is_none());
        assert!(dirs_under_override(None).is_none());
    }

    #[test]
    fn notice_is_returned_once() {
        let notice = StartupNotice(Mutex::new(Some("library.db.bad".to_string())));
        assert_eq!(notice.take().as_deref(), Some("library.db.bad"));
        assert_eq!(notice.take(), None);
    }

    #[test]
    fn healthy_db_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        for _ in 0..2 {
            match open_or_quarantine(&path).unwrap() {
                OpenOutcome::Opened(db) => assert_eq!(user_version(&db), MIGRATIONS.len() as i64),
                OpenOutcome::Quarantined { .. } => panic!("healthy DB was quarantined"),
            }
        }
        assert!(!dir.path().join("library.db.bad").exists());
    }

    #[test]
    fn corrupt_db_is_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        let garbage = vec![0xA5_u8; 4096];
        std::fs::write(&path, &garbage).unwrap();
        match open_or_quarantine(&path).unwrap() {
            OpenOutcome::Quarantined { db, bad } => {
                assert_eq!(bad, dir.path().join("library.db.bad"));
                assert_eq!(std::fs::read(&bad).unwrap(), garbage);
                assert_eq!(user_version(&db), MIGRATIONS.len() as i64);
            }
            OpenOutcome::Opened(_) => panic!("corrupt DB was opened"),
        }
    }

    #[test]
    fn second_quarantine_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        std::fs::write(dir.path().join("library.db.bad"), b"first").unwrap();
        std::fs::write(&path, vec![0x5A_u8; 4096]).unwrap();
        match open_or_quarantine(&path).unwrap() {
            OpenOutcome::Quarantined { bad, .. } => {
                assert_eq!(bad, dir.path().join("library.db.bad.2"));
            }
            OpenOutcome::Opened(_) => panic!("corrupt DB was opened"),
        }
        assert_eq!(
            std::fs::read(dir.path().join("library.db.bad")).unwrap(),
            b"first"
        );
    }

    #[test]
    fn newer_schema_is_an_error_not_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99_i64).unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        let err = open_or_quarantine(&path).err().expect("must fail");
        assert!(err.downcast_ref::<crate::db::NewerSchema>().is_some());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!dir.path().join("library.db.bad").exists());
        assert!(backups(dir.path()).is_empty());
    }

    #[test]
    fn backup_before_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        v1_db(&path);
        match open_or_quarantine(&path).unwrap() {
            OpenOutcome::Opened(db) => assert_eq!(user_version(&db), MIGRATIONS.len() as i64),
            OpenOutcome::Quarantined { .. } => panic!("healthy DB was quarantined"),
        }
        let name = format!("library.db.pre-v1-v{}", MIGRATIONS.len());
        assert_eq!(backups(dir.path()), vec![name.clone()]);
        let backup = dir.path().join(name);
        assert_eq!(version_of(&backup), 1);
        assert_eq!(count_tracks(&backup), 3);
    }

    #[test]
    fn no_backup_for_fresh_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        assert!(matches!(
            open_or_quarantine(&path).unwrap(),
            OpenOutcome::Opened(_)
        ));
        assert!(backups(dir.path()).is_empty());
    }

    #[test]
    fn keeps_three_backups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(1000);
        for (n, name) in ["pre-v1-v2", "pre-v0-v1", "pre-v5-v6", "pre-v2-v3"]
            .iter()
            .enumerate()
        {
            let file =
                std::fs::File::create(dir.path().join(format!("library.db.{name}"))).unwrap();
            file.set_modified(old + std::time::Duration::from_secs(n as u64 * 10))
                .unwrap();
        }
        v1_db(&path);
        open_or_quarantine(&path).unwrap();
        let kept = backups(dir.path());
        let new = format!("library.db.pre-v1-v{}", MIGRATIONS.len());
        assert_eq!(kept.len(), 3, "{kept:?}");
        assert!(kept.contains(&new), "{kept:?}");
        assert!(
            kept.contains(&"library.db.pre-v2-v3".to_string()),
            "{kept:?}"
        );
        assert!(
            kept.contains(&"library.db.pre-v5-v6".to_string()),
            "{kept:?}"
        );
    }
}
