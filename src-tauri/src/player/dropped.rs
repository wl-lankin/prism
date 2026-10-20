//! Files dropped onto the window play without being indexed. Their tags are read on the fly
//! and the tracks live in a small in-memory map under `file:<sha1 of the path>` ids, which
//! the player's resolver (on the opener thread) looks up before the library.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

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

    /// Reads the playable files among `paths` and returns their ids in the given order.
    /// Folders (M1), non-audio files and unreadable files are skipped.
    pub fn add(&self, paths: &[String]) -> Vec<String> {
        let mut tracks = Vec::new();
        for raw in paths {
            let path = Path::new(raw);
            if !path.is_file() || !is_audio_file(path) {
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
        let ids: Vec<String> = tracks.iter().map(|track| track.id.clone()).collect();
        let mut map = match self.tracks.lock() {
            Ok(map) => map,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !tracks.is_empty() {
            map.clear();
            map.extend(tracks.into_iter().map(|track| (track.id.clone(), track)));
        }
        ids
    }

    pub fn get(&self, id: &str) -> Option<Track> {
        let map = match self.tracks.lock() {
            Ok(map) => map,
            Err(poisoned) => poisoned.into_inner(),
        };
        map.get(id).cloned()
    }
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
mod tests {
    use super::*;
    use crate::library::tags::fixtures;

    fn path_of(file: &str) -> String {
        fixtures().join(file).to_string_lossy().into_owned()
    }

    #[test]
    fn reads_tags_without_indexing_and_resolves_by_file_id() {
        let dropped = Arc::new(DroppedFiles::new(None));
        let ids = dropped.add(&[path_of("tagged.mp3"), path_of("tagged.flac")]);
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
        let ids = dropped.add(&[
            text.to_string_lossy().into_owned(),
            dir.path().to_string_lossy().into_owned(),
            fixtures()
                .join("cover")
                .join("folder.jpg")
                .to_string_lossy()
                .into_owned(),
            path_of("tagged.mp3"),
        ]);
        assert_eq!(ids.len(), 1, "only the mp3 fixture is playable: {ids:?}");
        assert!(dropped
            .add(&[text.to_string_lossy().into_owned()])
            .is_empty());
        assert!(
            dropped.get(&ids[0]).is_some(),
            "a drop without playable files keeps the previous tracks"
        );
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
