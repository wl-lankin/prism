use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use walkdir::WalkDir;

use super::covers::{find_folder_cover, CoverStore};
use super::model::{track_id, Track};
use super::tags;
use crate::db::Db;

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "m4b", "aac", "alac", "flac", "wav", "aif", "aiff", "aifc", "ogg", "oga", "opus",
    "wma", "ape", "wv", "mpc", "tta", "dsf", "dff", "mka", "webm", "caf",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub done: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub added: u32,
    pub updated: u32,
    pub removed: u32,
    pub unchanged: u32,
    pub failed: Vec<(String, String)>,
}

/// Tracks are read and written in chunks of this size, one DB transaction each.
const CHUNK: usize = 500;

/// The key a folder is stored under in `folders.path` and `tracks.folder`.
pub fn folder_key(folder: &Path) -> String {
    folder.to_string_lossy().into_owned()
}

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext.as_str()))
}

/// `(mtime in ms since the epoch, size in bytes)`: a file is unchanged when both match.
pub fn file_stamp(meta: &Metadata) -> (i64, i64) {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_millis() as i64)
        .unwrap_or(0);
    (mtime, meta.len() as i64)
}

pub(crate) fn lock(db: &Db) -> anyhow::Result<MutexGuard<'_, Connection>> {
    db.0.lock()
        .map_err(|_| anyhow::anyhow!("library database lock poisoned"))
}

struct Indexed {
    path: Option<String>,
    album_id: String,
    mtime: Option<i64>,
    size: Option<i64>,
    duration_ms: i64,
    format: String,
}

impl Indexed {
    /// A row from a format only ffprobe can read that was indexed without ffprobe. Without
    /// ffprobe now (`reread_unparsed` false) re-reading could not improve it.
    fn needs_reread(&self, reread_unparsed: bool) -> bool {
        reread_unparsed
            && self.duration_ms == 0
            && tags::UNPARSED_EXTENSIONS.contains(&self.format.as_str())
    }
}

/// `path -> (mtime, size)` of files whose read failed, so an unchanged failing file is not
/// read again on every scan. In memory only: a restart tries each failing file once more.
pub type FailedStamps = HashMap<String, (i64, i64)>;

/// What a scan needs besides the database: decided once per `Library`.
pub struct ScanInputs<'a> {
    /// Re-read zero-duration rows of `UNPARSED_EXTENSIONS` (ffprobe is available).
    pub reread_unparsed: bool,
    pub failed_stamps: &'a Mutex<FailedStamps>,
}

/// A read track and its cover as `(id, file name)`, ready to be written under the lock.
type PreparedTrack = (Track, Option<(String, String)>);

struct Pending {
    id: String,
    path: PathBuf,
    mtime: i64,
    size: i64,
}

/// Indexes one registered library folder: new and changed files are read (in parallel),
/// unchanged ones are skipped, vanished ones removed. A missing or unreadable folder is
/// marked offline and its tracks are kept. A folder that is not registered in `folders`
/// (for example one just removed) is left alone and yields an empty report. Callers
/// serialize scans (see `Library`), so the registration check cannot race a removal.
///
/// A folder whose walk finds no audio file while it has indexed tracks is treated the same
/// way (an empty mount point of a NAS share looks exactly like that): it goes offline and
/// nothing is removed.
pub fn scan_folder(
    db: &Db,
    covers: &CoverStore,
    folder: &Path,
    inputs: &ScanInputs,
    progress: impl Fn(ScanProgress),
) -> anyhow::Result<ScanReport> {
    scan_folder_with(db, covers, folder, inputs, progress, &|dir| {
        std::fs::read_dir(dir).map(|_| ())
    })
}

/// The message of the `failed` entry a scan reports when the empty mount guard fires.
pub const EMPTY_FOLDER_OFFLINE: &str = "no audio files found; treated as offline";

/// `scan_folder` with the root listing check injectable (tests simulate a folder that
/// exists but cannot be listed).
fn scan_folder_with(
    db: &Db,
    covers: &CoverStore,
    folder: &Path,
    inputs: &ScanInputs,
    progress: impl Fn(ScanProgress),
    can_list: &dyn Fn(&Path) -> std::io::Result<()>,
) -> anyhow::Result<ScanReport> {
    let key = folder_key(folder);
    let mut report = ScanReport::default();

    if !is_registered(&*lock(db)?, &key)? {
        return Ok(report);
    }
    reconcile_albums(db)?;

    if !folder.is_dir() || can_list(folder).is_err() {
        let conn = lock(db)?;
        set_online(&conn, &key, false)?;
        return Ok(report);
    }

    let mut found: Vec<(PathBuf, i64, i64)> = Vec::new();
    let mut unreadable_dirs: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(folder).follow_links(true) {
        match entry {
            Ok(entry) if entry.file_type().is_file() && is_audio_file(entry.path()) => {
                match entry.metadata() {
                    Ok(meta) => {
                        let (mtime, size) = file_stamp(&meta);
                        found.push((entry.into_path(), mtime, size));
                    }
                    Err(err) => {
                        // Keep its indexed row: unreadable is not deleted.
                        unreadable_dirs.push(entry.path().to_path_buf());
                        report
                            .failed
                            .push((entry.path().display().to_string(), err.to_string()));
                    }
                }
            }
            Ok(_) => {}
            Err(err) => {
                // An error without a path is attributed to the whole folder, which keeps
                // every indexed track of this folder.
                let path = match err.path() {
                    Some(path) => path.to_path_buf(),
                    None => folder.to_path_buf(),
                };
                if err.loop_ancestor().is_none() {
                    unreadable_dirs.push(path.clone());
                }
                report
                    .failed
                    .push((path.display().to_string(), err.to_string()));
            }
        }
    }

    let indexed = load_indexed(db, &key)?;
    if found.is_empty() && !indexed.is_empty() {
        // Mass-deletion guard: an empty walk over a folder with indexed tracks is far more
        // likely an unmounted share or a failed listing than a deliberately emptied library.
        let conn = lock(db)?;
        set_online(&conn, &key, false)?;
        report.failed.push((key, EMPTY_FOLDER_OFFLINE.to_string()));
        return Ok(report);
    }
    let total = found.len() as u32;
    let mut done = 0u32;
    let mut seen: HashSet<String> = HashSet::with_capacity(found.len());
    let mut pending: Vec<Pending> = Vec::new();
    let failed_before: FailedStamps = inputs
        .failed_stamps
        .lock()
        .map_err(|_| anyhow::anyhow!("failed stamps lock poisoned"))?
        .clone();
    for (path, mtime, size) in found {
        // The walk yields paths below `folder`, so the prefix always strips.
        let relative = path.strip_prefix(folder).unwrap_or(&path);
        let id = track_id(&key, relative);
        if !seen.insert(id.clone()) {
            continue;
        }
        let indexed_unchanged = indexed.get(&id).is_some_and(|row| {
            row.mtime == Some(mtime)
                && row.size == Some(size)
                && !row.needs_reread(inputs.reread_unparsed)
        });
        // A file that failed to read and has not changed since fails again: skip it.
        let failed_unchanged = failed_before.get(&*path.to_string_lossy()) == Some(&(mtime, size));
        if indexed_unchanged || failed_unchanged {
            report.unchanged += 1;
            done += 1;
        } else {
            pending.push(Pending {
                id,
                path,
                mtime,
                size,
            });
        }
    }
    progress(ScanProgress { done, total });

    let mut folder_covers: HashMap<PathBuf, Option<String>> = HashMap::new();
    for chunk in pending.chunks(CHUNK) {
        let results: Vec<anyhow::Result<(Track, Option<String>)>> = chunk
            .par_iter()
            .map(|item| {
                let mut meta = tags::read(&item.path)?;
                meta.track.id = item.id.clone();
                // Cover order: embedded art first, then a cover file in the folder.
                let embedded = meta
                    .embedded_cover
                    .take()
                    .and_then(|bytes| covers.store_bytes(&bytes).ok());
                Ok((meta.track, embedded))
            })
            .collect();

        // Cover files are stat'ed, read and hashed before the database lock is taken; only
        // the id and file name go into the transaction.
        let prepared: Vec<anyhow::Result<PreparedTrack>> = chunk
            .iter()
            .zip(results)
            .map(|(item, result)| {
                let (mut track, embedded) = result?;
                track.cover_id =
                    embedded.or_else(|| folder_cover(covers, &mut folder_covers, &item.path));
                let cover = track
                    .cover_id
                    .as_ref()
                    .map(|id| (id.clone(), cover_file_name(covers, id)));
                Ok((track, cover))
            })
            .collect();

        let mut conn = lock(db)?;
        let tx = conn.transaction()?;
        // Albums are rebuilt in the same transaction as their tracks, so an interrupted
        // scan never leaves committed tracks without their album rows.
        let mut touched_albums: HashSet<String> = HashSet::new();
        let mut stamps = inputs
            .failed_stamps
            .lock()
            .map_err(|_| anyhow::anyhow!("failed stamps lock poisoned"))?;
        for (item, result) in chunk.iter().zip(prepared) {
            let path_key = item.path.to_string_lossy().into_owned();
            let (track, cover) = match result {
                Ok(read) => read,
                Err(err) => {
                    stamps.insert(path_key, (item.mtime, item.size));
                    report
                        .failed
                        .push((item.path.display().to_string(), format!("{err:#}")));
                    continue;
                }
            };
            stamps.remove(&path_key);
            if let Some((cover_id, file)) = &cover {
                register_cover(&tx, cover_id, file)?;
            }
            upsert_track(&tx, &track, &key, item.mtime, item.size)?;
            if let Some(previous) = indexed.get(&item.id) {
                touched_albums.insert(previous.album_id.clone());
                report.updated += 1;
            } else {
                report.added += 1;
            }
            touched_albums.insert(track.album_id);
        }
        rebuild_albums(&tx, touched_albums.iter())?;
        tx.commit()?;
        done += chunk.len() as u32;
        progress(ScanProgress { done, total });
    }

    // Forget failing files of this folder that are gone.
    inputs
        .failed_stamps
        .lock()
        .map_err(|_| anyhow::anyhow!("failed stamps lock poisoned"))?
        .retain(|path, _| !Path::new(path).starts_with(folder) || Path::new(path).exists());

    let mut conn = lock(db)?;
    let tx = conn.transaction()?;
    let mut touched_albums: HashSet<String> = HashSet::new();
    for (id, row) in &indexed {
        if seen.contains(id) {
            continue;
        }
        // Files below a directory we could not read are kept, not treated as deleted.
        let below_unreadable = row.path.as_ref().is_some_and(|path| {
            unreadable_dirs
                .iter()
                .any(|dir| Path::new(path).starts_with(dir))
        });
        if below_unreadable {
            continue;
        }
        tx.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
        touched_albums.insert(row.album_id.clone());
        report.removed += 1;
    }
    rebuild_albums(&tx, touched_albums.iter())?;
    set_online(&tx, &key, true)?;
    tx.commit()?;
    Ok(report)
}

fn load_indexed(db: &Db, key: &str) -> anyhow::Result<HashMap<String, Indexed>> {
    let conn = lock(db)?;
    let mut stmt = conn.prepare(
        "SELECT id, path, album_id, mtime, size, duration_ms, format FROM tracks WHERE folder = ?1",
    )?;
    let rows = stmt.query_map([key], |row| {
        Ok((
            row.get::<_, String>(0)?,
            Indexed {
                path: row.get(1)?,
                album_id: row.get(2)?,
                mtime: row.get(3)?,
                size: row.get(4)?,
                duration_ms: row.get(5)?,
                format: row.get(6)?,
            },
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Updates the online flag of a registered folder. Never registers one.
fn set_online(conn: &Connection, key: &str, online: bool) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE folders SET online = ?2 WHERE path = ?1",
        params![key, online as i64],
    )?;
    Ok(())
}

pub(crate) fn is_registered(conn: &Connection, key: &str) -> anyhow::Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM folders WHERE path = ?1", [key], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Creates album rows for tracks whose album is missing, e.g. after a crash between a
/// track write and its album rebuild in older builds or an aborted scan.
fn reconcile_albums(db: &Db) -> anyhow::Result<()> {
    let mut conn = lock(db)?;
    let missing: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT album_id FROM tracks WHERE album_id NOT IN (SELECT id FROM albums)",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    if missing.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    rebuild_albums(&tx, missing.iter())?;
    tx.commit()?;
    Ok(())
}

/// The cover file next to `track_path`, stored once per directory per scan.
fn folder_cover(
    covers: &CoverStore,
    cache: &mut HashMap<PathBuf, Option<String>>,
    track_path: &Path,
) -> Option<String> {
    let dir = track_path.parent()?.to_path_buf();
    cache
        .entry(dir)
        .or_insert_with_key(|dir| {
            let file = find_folder_cover(dir)?;
            let bytes = std::fs::read(file).ok()?;
            covers.store_bytes(&bytes).ok()
        })
        .clone()
}

/// The file name a stored cover has on disk (stats the cover directory).
fn cover_file_name(covers: &CoverStore, id: &str) -> String {
    covers
        .path(id)
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| id.to_string())
}

fn register_cover(conn: &Connection, id: &str, file: &str) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO covers (id, file) VALUES (?1, ?2)
         ON CONFLICT(id) DO UPDATE SET file = excluded.file",
        params![id, file],
    )?;
    Ok(())
}

/// Upsert keyed on `id`. Never `INSERT OR REPLACE`: that bypasses the FTS delete trigger.
fn upsert_track(
    conn: &Connection,
    track: &Track,
    folder: &str,
    mtime: i64,
    size: i64,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id, track_no,
            disc_no, duration_ms, year, genre, format, sample_rate, bit_depth, bitrate_kbps,
            cover_id, path, replay_gain_track_db, replay_gain_album_db, replay_gain_track_peak,
            replay_gain_album_peak, folder, mtime, size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
            ?19, ?20, ?21, ?22, ?23, ?24, ?25)
         ON CONFLICT(id) DO UPDATE SET
            source = excluded.source, title = excluded.title, artist = excluded.artist,
            album_artist = excluded.album_artist, album = excluded.album,
            album_id = excluded.album_id, track_no = excluded.track_no,
            disc_no = excluded.disc_no, duration_ms = excluded.duration_ms, year = excluded.year,
            genre = excluded.genre, format = excluded.format, sample_rate = excluded.sample_rate,
            bit_depth = excluded.bit_depth, bitrate_kbps = excluded.bitrate_kbps,
            cover_id = excluded.cover_id, path = excluded.path,
            replay_gain_track_db = excluded.replay_gain_track_db,
            replay_gain_album_db = excluded.replay_gain_album_db,
            replay_gain_track_peak = excluded.replay_gain_track_peak,
            replay_gain_album_peak = excluded.replay_gain_album_peak,
            folder = excluded.folder, mtime = excluded.mtime, size = excluded.size",
        params![
            track.id,
            track.source.as_str(),
            track.title,
            track.artist,
            track.album_artist,
            track.album,
            track.album_id,
            track.track_no.map(i64::from),
            track.disc_no.map(i64::from),
            track.duration_ms as i64,
            track.year,
            track.genre,
            track.format,
            track.sample_rate.map(i64::from),
            track.bit_depth.map(i64::from),
            track.bitrate_kbps.map(i64::from),
            track.cover_id,
            track.path,
            track.replay_gain_track_db.map(f64::from),
            track.replay_gain_album_db.map(f64::from),
            track.replay_gain_track_peak.map(f64::from),
            track.replay_gain_album_peak.map(f64::from),
            folder,
            mtime,
            size,
        ],
    )?;
    Ok(())
}

/// Recomputes the album rows for `album_ids` from their tracks, deleting empty albums.
/// The album cover is the cover of its first track (disc, track number, path) that has one.
pub(crate) fn rebuild_albums<'a>(
    conn: &Connection,
    album_ids: impl Iterator<Item = &'a String>,
) -> anyhow::Result<()> {
    let mut tracks = conn.prepare(
        "SELECT album, album_artist, year, cover_id, duration_ms FROM tracks
         WHERE album_id = ?1
         ORDER BY disc_no IS NULL, disc_no, track_no IS NULL, track_no, path",
    )?;
    for album_id in album_ids {
        let rows = tracks
            .query_map([album_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i32>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let Some((title, artist, _, _, _)) = rows.first() else {
            conn.execute("DELETE FROM albums WHERE id = ?1", [album_id])?;
            continue;
        };
        let year = rows.iter().find_map(|row| row.2);
        // No track has a cover: keep the one an online lookup found.
        let cover = match rows.iter().find_map(|row| row.3.clone()) {
            Some(cover) => Some(cover),
            None => conn
                .query_row(
                    "SELECT cover_id FROM cover_lookups WHERE album_id = ?1 AND status = 'found'",
                    [album_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten(),
        };
        let duration: i64 = rows.iter().map(|row| row.4).sum();
        conn.execute(
            "INSERT INTO albums (id, source, title, artist, year, cover_id, track_count, duration_ms)
             VALUES (?1, 'local', ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                source = excluded.source, title = excluded.title, artist = excluded.artist,
                year = excluded.year, cover_id = excluded.cover_id,
                track_count = excluded.track_count, duration_ms = excluded.duration_ms",
            params![album_id, title, artist, year, cover, rows.len() as i64, duration],
        )?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Instant;

    use rusqlite::params;
    use sha1::{Digest, Sha1};
    use tempfile::TempDir;

    use super::*;
    use crate::library::model::track_id;
    use crate::library::tags::fixtures;

    pub(crate) struct Harness {
        pub db: Db,
        pub covers: CoverStore,
        pub failed_stamps: Mutex<FailedStamps>,
        pub reread_unparsed: bool,
        _cover_dir: TempDir,
    }

    pub(crate) fn harness() -> Harness {
        let cover_dir = tempfile::tempdir().unwrap();
        Harness {
            db: Db::open_in_memory().unwrap(),
            covers: CoverStore::new(cover_dir.path().join("covers")),
            failed_stamps: Mutex::new(FailedStamps::new()),
            reread_unparsed: true,
            _cover_dir: cover_dir,
        }
    }

    impl Harness {
        pub fn inputs(&self) -> ScanInputs<'_> {
            ScanInputs {
                reread_unparsed: self.reread_unparsed,
                failed_stamps: &self.failed_stamps,
            }
        }

        /// Registers the folder (scans skip unregistered folders), then scans it.
        pub fn scan(&self, folder: &Path) -> ScanReport {
            self.db
                .0
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO folders (path, online) VALUES (?1, 1) ON CONFLICT(path) DO NOTHING",
                    [folder_key(folder)],
                )
                .unwrap();
            scan_folder(&self.db, &self.covers, folder, &self.inputs(), |_| {}).unwrap()
        }

        fn insert_row(
            &self,
            id: &str,
            path: &Path,
            folder: &Path,
            album_id: &str,
            extra: (i64, i64, i64, &str),
        ) {
            let (mtime, size, duration_ms, format) = extra;
            self.db
                .0
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                        duration_ms, format, path, folder, mtime, size)
                     VALUES (?1, 'local', 'Orphan', 'Orphan Artist', 'Orphan Artist', 'Orphan Album',
                        ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        id,
                        album_id,
                        duration_ms,
                        format,
                        path.to_string_lossy(),
                        folder_key(folder),
                        mtime,
                        size
                    ],
                )
                .unwrap();
        }

        pub fn count(&self, sql: &str) -> i64 {
            self.db
                .0
                .lock()
                .unwrap()
                .query_row(sql, [], |row| row.get(0))
                .unwrap()
        }

        fn cover_of_track(&self, title: &str) -> Option<String> {
            self.db
                .0
                .lock()
                .unwrap()
                .query_row(
                    "SELECT cover_id FROM tracks WHERE title = ?1",
                    [title],
                    |row| row.get(0),
                )
                .unwrap()
        }
    }

    /// Copies the fixture tree into a fresh temp dir, optionally leaving some files out.
    pub(crate) fn copy_fixtures(skip: &[&str]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(&fixtures(), dir.path(), skip);
        dir
    }

    fn copy_dir(from: &Path, to: &Path, skip: &[&str]) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if skip.contains(&name.to_string_lossy().as_ref()) {
                continue;
            }
            let target = to.join(&name);
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target, skip);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn embedded_cover_preferred() {
        let h = harness();
        h.scan(&fixtures());
        let cover: Option<String> =
            h.db.0
                .lock()
                .unwrap()
                .query_row(
                    "SELECT cover_id FROM albums WHERE title = 'Fixture Album'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
        let cover = cover.expect("album cover");
        let path = h.covers.path(&cover).expect("cover file");
        assert!(path.exists());
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(h.cover_of_track("Beta").as_deref(), Some(cover.as_str()));
        assert_eq!(
            h.count("SELECT COUNT(*) FROM covers WHERE id IN (SELECT cover_id FROM albums)"),
            h.count("SELECT COUNT(DISTINCT cover_id) FROM albums WHERE cover_id IS NOT NULL")
        );
    }

    #[test]
    fn folder_cover_used() {
        let h = harness();
        h.scan(&fixtures());
        let bytes = std::fs::read(fixtures().join("cover").join("folder.jpg")).unwrap();
        let expected = hex::encode(Sha1::digest(&bytes));
        assert_eq!(h.cover_of_track("Gamma"), Some(expected.clone()));
        assert_eq!(
            h.covers.path(&expected).unwrap().extension().unwrap(),
            "jpg"
        );
    }

    #[test]
    fn corrupt_file_reported_not_fatal() {
        let h = harness();
        let report = h.scan(&fixtures());
        let corrupt = fixtures().join("corrupt.flac");
        let corrupt_id = id_in(&fixtures(), &corrupt);
        let failed = report
            .failed
            .iter()
            .any(|(path, _)| path.ends_with("corrupt.flac"));
        let indexed_empty = h.count(&format!(
            "SELECT COUNT(*) FROM tracks WHERE id = '{corrupt_id}' AND duration_ms = 0"
        )) == 1;
        assert!(failed || indexed_empty, "{report:?}");
        for title in ["Alpha", "Beta", "notags", "Caf\u{e9}", "Gamma"] {
            assert_eq!(
                h.count(&format!(
                    "SELECT COUNT(*) FROM tracks WHERE title = '{title}'"
                )),
                1,
                "{title}"
            );
        }
        assert!(h.count("SELECT COUNT(*) FROM tracks") >= 10);
    }

    #[test]
    fn offline_folder_keeps_tracks() {
        let h = harness();
        let dir = copy_fixtures(&[]);
        let folder = dir.path().to_path_buf();
        h.scan(&folder);
        let before = h.count("SELECT COUNT(*) FROM tracks");
        assert!(before >= 10);
        assert_eq!(h.count("SELECT online FROM folders"), 1);
        dir.close().unwrap();
        let report = h.scan(&folder);
        assert_eq!(report.removed, 0);
        assert_eq!(h.count("SELECT COUNT(*) FROM tracks"), before);
        assert_eq!(h.count("SELECT online FROM folders"), 0);
    }

    /// The id `scan_folder` gives `path` inside the library folder `folder`.
    pub(crate) fn id_in(folder: &Path, path: &Path) -> String {
        track_id(&folder_key(folder), path.strip_prefix(folder).unwrap())
    }

    #[test]
    fn empty_mount_point_keeps_tracks() {
        let h = harness();
        let dir = copy_fixtures(&["corrupt.flac"]);
        let folder = dir.path().to_path_buf();
        h.scan(&folder);
        let before = h.count("SELECT COUNT(*) FROM tracks");
        assert!(before >= 10);
        // A NAS mount point that is present but empty: the directory exists, the files do not.
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path).unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
        }
        assert!(folder.is_dir());
        let report = h.scan(&folder);
        assert_eq!(report.removed, 0, "{report:?}");
        assert_eq!(h.count("SELECT COUNT(*) FROM tracks"), before);
        assert_eq!(h.count("SELECT online FROM folders"), 0);
        assert_eq!(
            report.failed,
            vec![(folder_key(&folder), EMPTY_FOLDER_OFFLINE.to_string())]
        );
    }

    #[test]
    fn empty_folder_without_tracks_stays_online() {
        let h = harness();
        let dir = tempfile::tempdir().unwrap();
        let report = h.scan(dir.path());
        assert_eq!(report, ScanReport::default());
        assert_eq!(h.count("SELECT online FROM folders"), 1);
    }

    #[test]
    fn unreadable_existing_folder_keeps_tracks() {
        let h = harness();
        let dir = copy_fixtures(&["corrupt.flac"]);
        let folder = dir.path().to_path_buf();
        h.scan(&folder);
        let before = h.count("SELECT COUNT(*) FROM tracks");
        assert!(before >= 10);
        assert!(folder.is_dir());
        let denied = |_: &Path| -> std::io::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "access denied",
            ))
        };
        let report =
            scan_folder_with(&h.db, &h.covers, &folder, &h.inputs(), |_| {}, &denied).unwrap();
        assert_eq!(report.removed, 0);
        assert_eq!(h.count("SELECT COUNT(*) FROM tracks"), before);
        assert_eq!(h.count("SELECT online FROM folders"), 0);
    }

    #[test]
    fn rescan_after_rekey_is_unchanged() {
        let h = harness();
        let dir = copy_fixtures(&["corrupt.flac"]);
        let first = h.scan(dir.path());
        assert!(first.added >= 10, "{first:?}");
        let total = h.count("SELECT COUNT(*) FROM tracks");
        {
            let mut conn = h.db.0.lock().unwrap();
            let tx = conn.transaction().unwrap();
            // Back to M1-style ids, then through the migration step.
            tx.execute("UPDATE tracks SET id = 'local:m1-' || row_id", [])
                .unwrap();
            crate::db::rekey::migrate_0003(&tx).unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(
            h.count("SELECT COUNT(*) FROM tracks WHERE id LIKE 'local:m1-%'"),
            0
        );
        let second = h.scan(dir.path());
        assert_eq!(second.added, 0, "{second:?}");
        assert_eq!(second.updated, 0, "{second:?}");
        assert_eq!(second.removed, 0, "{second:?}");
        assert_eq!(second.unchanged as i64, total);
    }

    #[test]
    fn rescan_skips_unchanged() {
        let h = harness();
        let dir = copy_fixtures(&["corrupt.flac"]);
        let first = h.scan(dir.path());
        assert!(first.added >= 10, "{first:?}");
        let last = std::cell::Cell::new(None);
        let second = scan_folder(&h.db, &h.covers, dir.path(), &h.inputs(), |p| {
            last.set(Some(p))
        })
        .unwrap();
        let total = last.get().expect("progress reported").total;
        assert_eq!(second.added, 0);
        assert_eq!(second.updated, 0);
        assert_eq!(second.removed, 0);
        assert_eq!(second.unchanged, total);
        assert_eq!(second.unchanged, first.added);
    }

    #[test]
    fn rescan_detects_changes_and_removals() {
        let h = harness();
        let dir = copy_fixtures(&["corrupt.flac"]);
        h.scan(dir.path());
        std::fs::remove_file(dir.path().join("tagged.mp3")).unwrap();
        let wv = dir.path().join("wv.wv");
        let mut bytes = std::fs::read(&wv).unwrap();
        bytes.extend_from_slice(&[0; 16]);
        std::fs::write(&wv, bytes).unwrap();
        let report = h.scan(dir.path());
        assert_eq!(report.removed, 1);
        assert_eq!(report.updated, 1);
        assert_eq!(report.added, 0);
        assert_eq!(
            h.count("SELECT COUNT(*) FROM tracks WHERE title = 'Alpha'"),
            0
        );
        assert_eq!(
            h.count("SELECT track_count FROM albums WHERE title = 'Fixture Album'"),
            1
        );
        assert_eq!(
            h.count("SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH 'alpha'"),
            0
        );
    }

    #[test]
    fn albums_missing_after_an_interrupted_scan_are_rebuilt() {
        let h = harness();
        let dir = copy_fixtures(&["corrupt.flac"]);
        h.scan(dir.path());
        // A track committed by a chunk whose album row was never written (scan interrupted).
        let other = tempfile::tempdir().unwrap();
        h.insert_row(
            "local:orphan",
            &other.path().join("x.mp3"),
            other.path(),
            "local:orphan-album",
            (0, 0, 1000, "mp3"),
        );
        assert_eq!(
            h.count("SELECT COUNT(*) FROM albums WHERE id = 'local:orphan-album'"),
            0
        );
        let report = h.scan(dir.path());
        assert_eq!(report.added + report.updated + report.removed, 0);
        assert_eq!(
            h.count("SELECT track_count FROM albums WHERE id = 'local:orphan-album'"),
            1
        );
    }

    #[test]
    fn unregistered_folder_is_not_scanned() {
        let h = harness();
        let dir = copy_fixtures(&["corrupt.flac"]);
        let report = scan_folder(&h.db, &h.covers, dir.path(), &h.inputs(), |_| {}).unwrap();
        assert_eq!(report, ScanReport::default());
        assert_eq!(h.count("SELECT COUNT(*) FROM folders"), 0);
        assert_eq!(h.count("SELECT COUNT(*) FROM tracks"), 0);
        let missing = dir.path().join("missing");
        scan_folder(&h.db, &h.covers, &missing, &h.inputs(), |_| {}).unwrap();
        assert_eq!(h.count("SELECT COUNT(*) FROM folders"), 0);
    }

    #[test]
    fn zero_duration_rows_of_unparsed_formats_are_reread() {
        let h = harness();
        let dir = copy_fixtures(&["corrupt.flac"]);
        h.scan(dir.path());
        let wma = dir.path().join("wma.wma");
        h.db.0
            .lock()
            .unwrap()
            .execute(
                "UPDATE tracks SET duration_ms = 0 WHERE id = ?1",
                [id_in(dir.path(), &wma)],
            )
            .unwrap();
        let report = h.scan(dir.path());
        assert_eq!(report.updated, 1, "{report:?}");
        assert_eq!(report.added + report.removed, 0);
    }

    #[test]
    fn failing_file_not_reread_when_unchanged() {
        let h = harness();
        let dir = copy_fixtures(&[]);
        let corrupt = dir.path().join("corrupt.flac");
        let first = h.scan(dir.path());
        assert!(
            first
                .failed
                .iter()
                .any(|(path, _)| path.ends_with("corrupt.flac")),
            "{first:?}"
        );
        let second = h.scan(dir.path());
        assert!(second.failed.is_empty(), "{second:?}");
        assert_eq!(second.unchanged, first.added + 1, "{second:?}");
        assert_eq!(second.added + second.updated + second.removed, 0);

        // A changed file is read (and fails) again.
        let mut bytes = std::fs::read(&corrupt).unwrap();
        bytes.extend_from_slice(&[0; 8]);
        std::fs::write(&corrupt, bytes).unwrap();
        let third = h.scan(dir.path());
        assert!(
            third
                .failed
                .iter()
                .any(|(path, _)| path.ends_with("corrupt.flac")),
            "{third:?}"
        );
    }

    #[test]
    fn unparsed_rows_are_left_alone_without_ffprobe() {
        let mut h = harness();
        h.reread_unparsed = false;
        let dir = copy_fixtures(&["corrupt.flac"]);
        h.scan(dir.path());
        let wma = dir.path().join("wma.wma");
        h.db.0
            .lock()
            .unwrap()
            .execute(
                "UPDATE tracks SET duration_ms = 0 WHERE id = ?1",
                [id_in(dir.path(), &wma)],
            )
            .unwrap();
        let report = h.scan(dir.path());
        assert_eq!(report.updated, 0, "{report:?}");
    }

    #[test]
    #[cfg_attr(debug_assertions, ignore = "wall-clock bound: run with --release")]
    fn rescan_2000_unchanged_under_1s() {
        let h = harness();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        let key = folder_key(&folder);
        let files: Vec<PathBuf> = (0..2000)
            .map(|i| {
                let path = folder.join(format!("track-{i:04}.mp3"));
                std::fs::write(&path, b"").unwrap();
                path
            })
            .collect();
        {
            let mut conn = h.db.0.lock().unwrap();
            let tx = conn.transaction().unwrap();
            tx.execute("INSERT INTO folders (path, online) VALUES (?1, 1)", [&key])
                .unwrap();
            for (i, path) in files.iter().enumerate() {
                let (mtime, size) = file_stamp(&std::fs::metadata(path).unwrap());
                tx.execute(
                    "INSERT INTO tracks (id, source, title, artist, album_artist, album, album_id,
                        duration_ms, format, path, folder, mtime, size)
                     VALUES (?1, 'local', ?2, 'A', 'A', 'B', 'local:b', 1000, 'mp3', ?3, ?4, ?5, ?6)",
                    params![
                        id_in(&folder, path),
                        format!("Track {i}"),
                        path.to_string_lossy(),
                        key,
                        mtime,
                        size
                    ],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let started = Instant::now();
        let report = h.scan(&folder);
        let elapsed = started.elapsed();
        assert_eq!(report.unchanged, 2000, "{report:?}");
        assert_eq!(report.added + report.updated + report.removed, 0);
        assert!(elapsed.as_millis() < 1000, "rescan took {elapsed:?}");
    }
}
