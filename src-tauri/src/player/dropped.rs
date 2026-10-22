//! Files dropped onto the window play without being indexed. Their tags are read on the fly
//! and the tracks live in a small in-memory map under `file:<sha1 of the path>` ids, which
//! the player's resolver (on the opener thread) looks up before the library.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use super::engine::Resolver;
use crate::library::covers::CoverStore;
use crate::library::model::{file_track_id, Track};
use crate::library::scan::is_audio_file;
use crate::library::tags;

/// Prefix of the ids of dropped files.
pub const DROPPED_PREFIX: &str = "file:";

/// The tracks of the latest drop. Each drop replaces the previous one (it also replaces the
/// queue), so the map never holds more than one drop's worth of tracks.
pub struct DroppedFiles {
    tracks: Mutex<HashMap<String, Track>>,
    covers: Option<Arc<CoverStore>>,
}

impl DroppedFiles {
    /// `covers` receives embedded cover art so the UI can show it; `None` skips covers.
    pub fn new(covers: Option<Arc<CoverStore>>) -> DroppedFiles {
        DroppedFiles {
            tracks: Mutex::new(HashMap::new()),
            covers,
        }
    }

    /// Reads the playable files among `paths`, in the given order, without touching the map.
    /// Folders (M1), relative and network paths, non-audio files and unreadable files are skipped.
    pub fn read(&self, paths: &[String]) -> Vec<Track> {
        self.read_with(paths, ReadOptions::default())
    }

    /// Like [`DroppedFiles::read`]; `options` says what to allow. "Open with" allows network
    /// paths (the user picked that file in Explorer), drag and drop does not.
    pub fn read_with(&self, paths: &[String], options: ReadOptions) -> Vec<Track> {
        let mut tracks = Vec::new();
        for raw in paths {
            // Checked before any file system access: a share can hang on a slow network.
            if !options.allow_network && is_network_path(raw) {
                continue;
            }
            let path = Path::new(raw);
            if !path.is_absolute() || !path.is_file() || !is_audio_file(path) {
                continue;
            }
            let meta = match tags::read(path) {
                Ok(meta) => meta,
                Err(err) => {
                    eprintln!("a dropped file cannot be read: {err:#}");
                    continue;
                }
            };
            let mut track = meta.track;
            track.id = file_track_id(path);
            track.path = Some(raw.clone());
            if track.cover_id.is_none() {
                if let (Some(store), Some(bytes)) = (&self.covers, meta.embedded_cover.as_deref()) {
                    track.cover_id = store.store_bytes(bytes).ok();
                }
            }
            tracks.push(track);
        }
        tracks
    }

    /// Makes `tracks` the current drop and hands their ids to `play`. The map is committed
    /// only when `play` succeeds; on an error the previous drop is restored. With no tracks,
    /// `play` is not called and nothing changes. Returns how many tracks were queued.
    pub fn commit_and_play(
        &self,
        tracks: Vec<Track>,
        play: impl FnOnce(Vec<String>) -> Result<(), String>,
    ) -> Result<u32, String> {
        if tracks.is_empty() {
            return Ok(0);
        }
        let ids: Vec<String> = tracks.iter().map(|track| track.id.clone()).collect();
        let next: HashMap<String, Track> = tracks
            .into_iter()
            .map(|track| (track.id.clone(), track))
            .collect();
        // In the map before the command is sent: the opener thread may resolve at once.
        let previous = std::mem::replace(&mut *self.lock(), next);
        let count = ids.len() as u32;
        match play(ids) {
            Ok(()) => Ok(count),
            Err(err) => {
                *self.lock() = previous;
                Err(err)
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<Track> {
        self.lock().get(id).cloned()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Track>> {
        match self.tracks.lock() {
            Ok(map) => map,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// What [`DroppedFiles::read_with`] lets through beyond local audio files.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadOptions {
    pub allow_network: bool,
}

/// UNC shares (`\\server\share`), their verbatim form (`\\?\UNC\server\share`), device
/// paths (`\\.\...`) and mapped network drives (`Z:\...` on Windows). Dropped files are
/// played from local disks only.
pub fn is_network_path(raw: &str) -> bool {
    let path = raw.replace('/', r"\");
    let lower = path.to_ascii_lowercase();
    if lower.starts_with(r"\\?\unc\") {
        return true;
    }
    if lower.starts_with(r"\\?\") {
        // A verbatim local path such as `\\?\C:\Music`.
        return false;
    }
    if path.starts_with(r"\\") {
        return true;
    }
    drive_root(&path).is_some_and(|root| is_remote_drive(&root))
}

/// `C:\` for a path that starts with a drive letter.
fn drive_root(path: &str) -> Option<String> {
    let mut chars = path.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    (chars.next() == Some(':')).then(|| format!("{}:\\", letter.to_ascii_uppercase()))
}

/// Whether the drive at `root` (`Z:\`) is a mapped network drive. Asks only the drive
/// letter's type, which does not touch the share.
#[cfg(target_os = "windows")]
fn is_remote_drive(root: &str) -> bool {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

    const DRIVE_REMOTE: u32 = 4;
    let wide: Vec<u16> = std::ffi::OsStr::new(root)
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe { GetDriveTypeW(wide.as_ptr()) == DRIVE_REMOTE }
}

#[cfg(not(target_os = "windows"))]
fn is_remote_drive(_root: &str) -> bool {
    false
}

/// A resolver that answers `file:` ids from `dropped` and everything else from `fallback`.
pub fn with_dropped(dropped: Arc<DroppedFiles>, fallback: Resolver) -> Resolver {
    Arc::new(move |id: &str| {
        if id.starts_with(DROPPED_PREFIX) {
            dropped.get(id)
        } else {
            fallback(id)
        }
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::library::tags::fixtures;

    fn path_of(file: &str) -> String {
        fixtures().join(file).to_string_lossy().into_owned()
    }

    /// Reads `paths` and commits them with a player that accepts; returns the queued ids.
    pub(crate) fn drop_files(dropped: &DroppedFiles, paths: &[String]) -> Vec<String> {
        let mut queued = Vec::new();
        dropped
            .commit_and_play(dropped.read(paths), |ids| {
                queued = ids;
                Ok(())
            })
            .unwrap();
        queued
    }

    #[test]
    fn reads_tags_without_indexing_and_resolves_by_file_id() {
        let dropped = Arc::new(DroppedFiles::new(None));
        let ids = drop_files(&dropped, &[path_of("tagged.mp3"), path_of("tagged.flac")]);
        assert_eq!(ids.len(), 2);
        assert!(ids.iter().all(|id| id.starts_with(DROPPED_PREFIX)));
        let resolver = with_dropped(Arc::clone(&dropped), Arc::new(|_id: &str| None));
        let first = resolver(&ids[0]).expect("the dropped file resolves");
        assert_eq!(first.title, "Alpha");
        assert_eq!(first.path.as_deref(), Some(path_of("tagged.mp3").as_str()));
        assert_eq!(resolver(&ids[1]).unwrap().title, "Beta");
    }

    #[test]
    fn ignores_non_audio_files_and_folders() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "not audio").unwrap();
        let fake = dir.path().join("fake.mp3");
        std::fs::write(&fake, "not audio either").unwrap();
        let dropped = DroppedFiles::new(None);
        let ids = drop_files(
            &dropped,
            &[
                text.to_string_lossy().into_owned(),
                fake.to_string_lossy().into_owned(),
                dir.path().to_string_lossy().into_owned(),
                fixtures()
                    .join("cover")
                    .join("folder.jpg")
                    .to_string_lossy()
                    .into_owned(),
                path_of("tagged.mp3"),
            ],
        );
        assert_eq!(ids.len(), 1, "only the mp3 fixture is playable: {ids:?}");
        let mut called = false;
        let count = dropped
            .commit_and_play(dropped.read(&[text.to_string_lossy().into_owned()]), |_| {
                called = true;
                Ok(())
            })
            .unwrap();
        assert_eq!(count, 0);
        assert!(!called, "nothing playable: the player is not touched");
        assert!(
            dropped.get(&ids[0]).is_some(),
            "a drop without playable files keeps the previous tracks"
        );
    }

    #[test]
    fn relative_paths_are_refused() {
        let dropped = DroppedFiles::new(None);
        let absolute = fixtures().join("tagged.mp3");
        let relative = absolute
            .strip_prefix(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(dropped.read(&[relative, "-x.flac".to_string()]).is_empty());
        assert_eq!(
            dropped
                .read(&[absolute.to_string_lossy().into_owned()])
                .len(),
            1
        );
    }

    #[test]
    fn network_paths_are_refused() {
        for path in [
            r"\\server\share\song.flac",
            "//server/share/song.flac",
            r"\\?\UNC\server\share\song.flac",
            r"\\?\unc\server\share\song.flac",
            r"\\.\pipe\song.flac",
        ] {
            assert!(is_network_path(path), "{path}");
        }
        for path in [
            r"C:\Music\song.flac",
            r"\\?\C:\Music\song.flac",
            "/Users/me/song.flac",
        ] {
            assert!(!is_network_path(path), "{path}");
        }
        let dropped = DroppedFiles::new(None);
        assert!(dropped
            .read(&[r"\\server\share\song.flac".to_string()])
            .is_empty());
    }

    #[test]
    fn drive_letters_are_found_without_touching_the_disk() {
        assert_eq!(drive_root(r"z:\music\a.flac").as_deref(), Some(r"Z:\"));
        assert_eq!(drive_root(r"C:rel.flac").as_deref(), Some(r"C:\"));
        assert_eq!(drive_root("/Users/me/a.flac"), None);
        assert_eq!(drive_root("rel.flac"), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn local_drives_are_not_network_drives() {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
        assert!(!is_network_path(&format!(r"{system}\Windows\notepad.exe")));
    }

    #[test]
    fn local_files_read_the_same_with_network_allowed() {
        let dropped = DroppedFiles::new(None);
        let local = vec![path_of("tagged.mp3")];
        let allowed = dropped.read_with(
            &local,
            ReadOptions {
                allow_network: true,
            },
        );
        assert_eq!(allowed.len(), dropped.read(&local).len());
        assert_eq!(allowed.len(), 1);
    }

    #[test]
    fn a_failed_play_restores_the_previous_drop() {
        let dropped = DroppedFiles::new(None);
        let first = drop_files(&dropped, &[path_of("tagged.mp3")]);
        let result = dropped.commit_and_play(dropped.read(&[path_of("tagged.flac")]), |_| {
            Err("The audio engine is not running.".to_string())
        });
        assert_eq!(result, Err("The audio engine is not running.".to_string()));
        assert!(
            dropped.get(&first[0]).is_some(),
            "the previous drop is back"
        );
        let flac = file_track_id(Path::new(&path_of("tagged.flac")));
        assert!(dropped.get(&flac).is_none(), "the failed drop was not kept");
    }

    #[test]
    fn other_ids_go_to_the_library() {
        let dropped = Arc::new(DroppedFiles::new(None));
        let resolver = with_dropped(
            dropped,
            Arc::new(|id: &str| {
                (id == "local:x").then(|| tags::read(&fixtures().join("tagged.mp3")).unwrap().track)
            }),
        );
        assert!(resolver("local:x").is_some());
        assert!(resolver("file:unknown").is_none());
    }
}
