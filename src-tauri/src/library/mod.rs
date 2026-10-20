pub mod covers;
pub mod model;
pub mod scan;
pub mod tags;
pub mod watch;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::db::Db;
use covers::CoverStore;
use scan::{folder_key, lock, rebuild_albums, scan_folder, ScanReport};

pub const EVENT_CHANGED: &str = "library://changed";
pub const EVENT_SCAN_PROGRESS: &str = "library://scan-progress";

/// Payload of `library://scan-progress`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderScanProgress {
    pub folder: String,
    pub done: u32,
    pub total: u32,
}

/// What the library tells the UI. The Tauri layer maps these onto `library://changed`
/// and `library://scan-progress`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryEvent {
    Changed,
    ScanProgress(FolderScanProgress),
}

/// The local library service: folders, scans and the events they produce.
pub struct Library {
    pub db: Arc<Db>,
    pub covers: Arc<CoverStore>,
    /// Scans never overlap: the watcher and user actions may ask for one at the same time.
    scan_lock: Mutex<()>,
}

impl Library {
    pub fn new(db: Arc<Db>, covers: Arc<CoverStore>) -> Library {
        Library {
            db,
            covers,
            scan_lock: Mutex::new(()),
        }
    }

    /// The library folders in the order they were added.
    pub fn folders(&self) -> anyhow::Result<Vec<PathBuf>> {
        let conn = lock(&self.db)?;
        let mut stmt = conn.prepare("SELECT path FROM folders ORDER BY rowid")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    /// Registers a folder and indexes it. The folder must exist.
    pub fn add_folder(
        &self,
        path: &Path,
        emit: &dyn Fn(LibraryEvent),
    ) -> anyhow::Result<ScanReport> {
        let path = normalize(path)?;
        if !path.is_dir() {
            anyhow::bail!("{} is not a folder", path.display());
        }
        lock(&self.db)?.execute(
            "INSERT INTO folders (path, online) VALUES (?1, 1) ON CONFLICT(path) DO NOTHING",
            [folder_key(&path)],
        )?;
        let report = self.scan(&path, emit)?;
        emit(LibraryEvent::Changed);
        Ok(report)
    }

    /// Forgets a folder and every track indexed from it.
    pub fn remove_folder(&self, path: &Path, emit: &dyn Fn(LibraryEvent)) -> anyhow::Result<()> {
        let _scan = self.scan_lock.lock().map_err(|_| poisoned())?;
        let key = folder_key(&normalize(path)?);
        let mut conn = lock(&self.db)?;
        let tx = conn.transaction()?;
        let albums: Vec<String> = {
            let mut stmt = tx.prepare("SELECT DISTINCT album_id FROM tracks WHERE folder = ?1")?;
            let rows = stmt.query_map([&key], |row| row.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        tx.execute("DELETE FROM tracks WHERE folder = ?1", [&key])?;
        rebuild_albums(&tx, albums.iter())?;
        tx.execute("DELETE FROM folders WHERE path = ?1", [&key])?;
        tx.commit()?;
        drop(conn);
        emit(LibraryEvent::Changed);
        Ok(())
    }

    /// Rescans one folder; emits `Changed` only when the index or its online state changed.
    pub fn rescan_folder(
        &self,
        path: &Path,
        emit: &dyn Fn(LibraryEvent),
    ) -> anyhow::Result<ScanReport> {
        let path = normalize(path)?;
        let key = folder_key(&path);
        let online_before = self.online(&key)?;
        let report = self.scan(&path, emit)?;
        let changed = report.added + report.updated + report.removed > 0;
        if changed || self.online(&key)? != online_before {
            emit(LibraryEvent::Changed);
        }
        Ok(report)
    }

    /// Rescans every folder. A failing folder does not stop the others.
    pub fn rescan_all(
        &self,
        emit: &dyn Fn(LibraryEvent),
    ) -> anyhow::Result<Vec<(String, ScanReport)>> {
        let mut reports = Vec::new();
        for folder in self.folders()? {
            let key = folder_key(&folder);
            match self.rescan_folder(&folder, emit) {
                Ok(report) => reports.push((key, report)),
                Err(err) => reports.push((
                    key.clone(),
                    ScanReport {
                        failed: vec![(key, format!("{err:#}"))],
                        ..ScanReport::default()
                    },
                )),
            }
        }
        Ok(reports)
    }

    /// Whether `path` is a library folder (after the same normalization as `add_folder`).
    pub fn is_registered(&self, path: &Path) -> anyhow::Result<bool> {
        let key = folder_key(&normalize(path)?);
        scan::is_registered(&*lock(&self.db)?, &key)
    }

    fn online(&self, key: &str) -> anyhow::Result<Option<bool>> {
        let conn = lock(&self.db)?;
        let online = conn
            .query_row("SELECT online FROM folders WHERE path = ?1", [key], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?;
        Ok(online.map(|value| value != 0))
    }

    fn scan(&self, path: &Path, emit: &dyn Fn(LibraryEvent)) -> anyhow::Result<ScanReport> {
        let _scan = self.scan_lock.lock().map_err(|_| poisoned())?;
        let folder = folder_key(path);
        scan_folder(&self.db, &self.covers, path, |progress| {
            emit(LibraryEvent::ScanProgress(FolderScanProgress {
                folder: folder.clone(),
                done: progress.done,
                total: progress.total,
            }))
        })
    }
}

fn poisoned() -> anyhow::Error {
    anyhow::anyhow!("library scan lock poisoned")
}

/// The one spelling a folder is stored under: absolute, as `std::path::absolute` gives it.
fn normalize(path: &Path) -> anyhow::Result<PathBuf> {
    Ok(std::path::absolute(path)?)
}

/// Emits library events to the webview as `library://changed` and `library://scan-progress`.
pub fn tauri_emitter(app: AppHandle) -> impl Fn(LibraryEvent) {
    move |event| {
        let result = match &event {
            LibraryEvent::Changed => app.emit(EVENT_CHANGED, ()),
            LibraryEvent::ScanProgress(progress) => app.emit(EVENT_SCAN_PROGRESS, progress),
        };
        if let Err(err) = result {
            eprintln!("emitting {event:?} failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::library::scan::tests::copy_fixtures;

    fn library() -> (Library, tempfile::TempDir) {
        let cover_dir = tempfile::tempdir().unwrap();
        let library = Library::new(
            Arc::new(Db::open_in_memory().unwrap()),
            Arc::new(CoverStore::new(cover_dir.path().to_path_buf())),
        );
        (library, cover_dir)
    }

    fn count(library: &Library, sql: &str) -> i64 {
        library
            .db
            .0
            .lock()
            .unwrap()
            .query_row(sql, [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn add_folder_scans_and_emits() {
        let (library, _covers) = library();
        let dir = copy_fixtures(&["corrupt.flac"]);
        let events = RefCell::new(Vec::new());
        let report = library
            .add_folder(dir.path(), &|event| events.borrow_mut().push(event))
            .unwrap();
        assert!(report.added >= 10);
        let events = events.into_inner();
        assert_eq!(events.last(), Some(&LibraryEvent::Changed));
        let last_progress = events
            .iter()
            .rev()
            .find_map(|event| match event {
                LibraryEvent::ScanProgress(progress) => Some(progress.clone()),
                LibraryEvent::Changed => None,
            })
            .expect("scan progress");
        assert_eq!(last_progress.done, last_progress.total);
        assert_eq!(last_progress.folder, folder_key(dir.path()));
        assert_eq!(library.folders().unwrap(), vec![dir.path().to_path_buf()]);
    }

    #[test]
    fn add_folder_rejects_missing_dir() {
        let (library, _covers) = library();
        let dir = tempfile::tempdir().unwrap();
        assert!(library
            .add_folder(&dir.path().join("nope"), &|_| {})
            .is_err());
        assert!(library.folders().unwrap().is_empty());
    }

    #[test]
    fn remove_folder_drops_tracks_and_albums() {
        let (library, _covers) = library();
        let first = copy_fixtures(&["corrupt.flac", "cover"]);
        let second = copy_fixtures(&["corrupt.flac"]);
        library.add_folder(first.path(), &|_| {}).unwrap();
        library.add_folder(second.path(), &|_| {}).unwrap();
        let both = count(&library, "SELECT COUNT(*) FROM tracks");
        let events = RefCell::new(Vec::new());
        library
            .remove_folder(second.path(), &|event| events.borrow_mut().push(event))
            .unwrap();
        assert_eq!(events.into_inner(), vec![LibraryEvent::Changed]);
        assert!(count(&library, "SELECT COUNT(*) FROM tracks") < both);
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM tracks WHERE title = 'Gamma'"
            ),
            0
        );
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM albums WHERE title = 'Cover Album'"
            ),
            0
        );
        assert_eq!(
            count(
                &library,
                "SELECT track_count FROM albums WHERE title = 'Fixture Album'"
            ),
            2
        );
        assert_eq!(library.folders().unwrap(), vec![first.path().to_path_buf()]);
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH 'gamma'"
            ),
            0
        );
    }

    #[test]
    fn removed_folder_does_not_come_back() {
        let (library, _covers) = library();
        let dir = copy_fixtures(&["corrupt.flac"]);
        library.add_folder(dir.path(), &|_| {}).unwrap();
        assert!(library.is_registered(dir.path()).unwrap());
        // A non-normalized spelling of the same folder must remove it too.
        let spelled = dir.path().join("cover").join("..");
        if std::path::absolute(&spelled).unwrap() == dir.path() {
            library.remove_folder(&spelled, &|_| {}).unwrap();
        } else {
            library.remove_folder(dir.path(), &|_| {}).unwrap();
        }
        assert!(!library.is_registered(dir.path()).unwrap());
        // The watcher (or anything else) rescanning the old path must not resurrect it.
        let events = RefCell::new(Vec::new());
        let report = library
            .rescan_folder(dir.path(), &|event| events.borrow_mut().push(event))
            .unwrap();
        assert_eq!(report, ScanReport::default());
        assert!(events.into_inner().is_empty());
        assert_eq!(count(&library, "SELECT COUNT(*) FROM folders"), 0);
        assert_eq!(count(&library, "SELECT COUNT(*) FROM tracks"), 0);
        assert_eq!(count(&library, "SELECT COUNT(*) FROM albums"), 0);
    }

    #[test]
    fn rescan_all_covers_every_folder_and_stays_quiet_when_unchanged() {
        let (library, _covers) = library();
        let first = copy_fixtures(&["corrupt.flac", "cover"]);
        let second = copy_fixtures(&["corrupt.flac"]);
        library.add_folder(first.path(), &|_| {}).unwrap();
        library.add_folder(second.path(), &|_| {}).unwrap();
        let events = RefCell::new(Vec::new());
        let reports = library
            .rescan_all(&|event| events.borrow_mut().push(event))
            .unwrap();
        assert_eq!(reports.len(), 2);
        assert!(reports
            .iter()
            .all(|(_, report)| report.unchanged > 0 && report.added == 0));
        assert!(!events.into_inner().contains(&LibraryEvent::Changed));

        std::fs::remove_file(second.path().join("tagged.mp3")).unwrap();
        let events = RefCell::new(Vec::new());
        library
            .rescan_all(&|event| events.borrow_mut().push(event))
            .unwrap();
        assert!(events.into_inner().contains(&LibraryEvent::Changed));
    }
}
