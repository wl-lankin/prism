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
        let mut tracks = Vec::new();
        for raw in paths {
            // Checked before any file system access: a share can hang on a slow network.
            if is_network_path(raw) {
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

/// UNC shares (`\\server\share`), their verbatim form (`\\?\UNC\server\share`) and device
/// paths (`\\.\...`). Dropped files are played from local disks only.
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
    path.starts_with(r"\\")
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
