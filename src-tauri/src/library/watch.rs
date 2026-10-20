use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use tauri::{AppHandle, Manager};

use super::covers::folder_cover_rank;
use super::scan::{self, is_audio_file};
use super::{tauri_emitter, Library, LibraryEvent};
use crate::AppState;

/// Changes are collected for this long before the affected folder is rescanned.
pub const DEBOUNCE: Duration = Duration::from_secs(2);

/// A watched library folder: the key it is stored under and its canonical form, which is
/// how some platforms report events (macOS FSEvents says `/private/var/...` for `/var/...`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchRoot {
    pub key: PathBuf,
    pub canonical: PathBuf,
}

impl WatchRoot {
    pub fn new(key: &Path) -> WatchRoot {
        WatchRoot {
            key: key.to_path_buf(),
            canonical: canonicalize_lossy(key),
        }
    }
}

/// Canonicalizes the longest existing ancestor of `path` and appends the rest, so paths of
/// deleted files still resolve. Falls back to `path` itself.
pub fn canonicalize_lossy(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if let Ok(canonical) = ancestor.canonicalize() {
            return match path.strip_prefix(ancestor) {
                Ok(rest) if !rest.as_os_str().is_empty() => canonical.join(rest),
                _ => canonical,
            };
        }
    }
    path.to_path_buf()
}

/// The library key of the folder that contains `path`: the longest matching root, compared
/// as given first and canonicalized second.
pub fn root_for<'a>(path: &Path, roots: &'a [WatchRoot]) -> Option<&'a PathBuf> {
    let longest = |matches: Vec<&'a WatchRoot>, by: fn(&WatchRoot) -> &PathBuf| {
        matches
            .into_iter()
            .max_by_key(|root| by(root).components().count())
            .map(|root| &root.key)
    };
    let raw: Vec<&WatchRoot> = roots.iter().filter(|r| path.starts_with(&r.key)).collect();
    if !raw.is_empty() {
        return longest(raw, |r| &r.key);
    }
    let canonical = canonicalize_lossy(path);
    let matched: Vec<&WatchRoot> = roots
        .iter()
        .filter(|r| canonical.starts_with(&r.canonical))
        .collect();
    longest(matched, |r| &r.canonical)
}

/// Whether a change to `path` can affect the index: an audio file, a folder cover file, or a
/// directory (which may have gained or lost either). Temp files and the like are ignored.
pub fn is_relevant(path: &Path, is_dir: bool) -> bool {
    is_dir || is_audio_file(path) || folder_cover_rank(path).is_some()
}

/// Extensions of files that are never audio or covers and are common as temp or side files.
const IGNORED_EXTENSIONS: &[&str] = &[
    "tmp",
    "part",
    "crdownload",
    "partial",
    "txt",
    "log",
    "nfo",
    "db",
    "ini",
    "m3u",
    "cue",
];

/// `is_relevant` for a path from a watcher event. A deleted path cannot be asked whether it
/// was a directory (and a folder name may contain a dot: "Vol. 2"), so a path that no longer
/// exists is relevant unless it has a known side-file extension: a spurious rescan is cheap.
fn event_is_relevant(path: &Path) -> bool {
    if path.exists() {
        return is_relevant(path, path.is_dir());
    }
    let ignored = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .is_some_and(|ext| IGNORED_EXTENSIONS.contains(&ext.as_str()));
    !ignored
}

/// Watches library folders recursively and reports, per debounce window, each library
/// folder that saw a change.
pub struct FolderWatcher {
    debouncer: Debouncer<RecommendedWatcher>,
    roots: Arc<Mutex<Vec<WatchRoot>>>,
}

impl FolderWatcher {
    pub fn new(
        debounce: Duration,
        on_change: impl Fn(PathBuf) + Send + 'static,
    ) -> anyhow::Result<FolderWatcher> {
        let roots: Arc<Mutex<Vec<WatchRoot>>> = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::clone(&roots);
        let debouncer = new_debouncer(debounce, move |result: DebounceEventResult| {
            let roots = match shared.lock() {
                Ok(roots) => roots.clone(),
                Err(_) => return,
            };
            let events = match result {
                Ok(events) => events,
                Err(err) => {
                    // Events were lost: rescan everything rather than miss a change.
                    eprintln!("folder watcher error: {err}");
                    for root in roots {
                        on_change(root.key);
                    }
                    return;
                }
            };
            let mut affected: Vec<PathBuf> = Vec::new();
            for event in events {
                if !event_is_relevant(&event.path) {
                    continue;
                }
                if let Some(root) = root_for(&event.path, &roots) {
                    if !affected.contains(root) {
                        affected.push(root.clone());
                    }
                }
            }
            for root in affected {
                on_change(root);
            }
        })
        .context("starting the folder watcher")?;
        Ok(FolderWatcher { debouncer, roots })
    }

    pub fn watch(&mut self, folder: &Path) -> anyhow::Result<()> {
        self.debouncer
            .watcher()
            .watch(folder, RecursiveMode::Recursive)
            .with_context(|| format!("watching {}", folder.display()))?;
        let mut roots = self.roots.lock().map_err(|_| poisoned())?;
        if !roots.iter().any(|root| root.key == folder) {
            roots.push(WatchRoot::new(folder));
        }
        Ok(())
    }

    /// Watches `folder` afresh: an OS watch on a folder that went away is dead, so any old
    /// one is dropped first.
    pub fn rewatch(&mut self, folder: &Path) -> anyhow::Result<()> {
        let _ = self.unwatch(folder);
        self.watch(folder)
    }

    pub fn unwatch(&mut self, folder: &Path) -> anyhow::Result<()> {
        self.roots
            .lock()
            .map_err(|_| poisoned())?
            .retain(|root| root.key != folder);
        self.debouncer
            .watcher()
            .unwatch(folder)
            .with_context(|| format!("unwatching {}", folder.display()))
    }
}

fn poisoned() -> anyhow::Error {
    anyhow::anyhow!("folder watcher lock poisoned")
}

/// The app-wide watcher, managed as Tauri state. `None` when the OS watcher failed to start.
pub struct WatchState(pub Mutex<Option<FolderWatcher>>);

/// How often offline folders are checked for coming back.
pub const OFFLINE_RECHECK: Duration = Duration::from_secs(60);

/// The names and modification times of a folder's direct entries, sorted: a cheap
/// fingerprint of its root listing.
pub type Listing = Vec<(String, Option<i64>)>;

/// What `recheck_offline` remembers per folder that is readable but still failed its scan.
pub type OfflineMemo = HashMap<PathBuf, Listing>;

fn root_listing(folder: &Path) -> std::io::Result<Listing> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        let mtime = entry.metadata().ok().map(|meta| scan::file_stamp(&meta).0);
        entries.push((entry.file_name().to_string_lossy().into_owned(), mtime));
    }
    entries.sort();
    Ok(entries)
}

/// Brings back offline folders that are readable again: such a folder is rescanned, and once
/// that brings it online it is watched (via `watch`) and `Changed` is emitted. A folder that
/// is readable but fails the scan again (an empty mount point) is remembered by its root
/// listing in `memo` and not rescanned until that listing changes. Returns the number of
/// scans run.
pub fn recheck_offline(
    library: &Library,
    memo: &mut OfflineMemo,
    watch: &mut dyn FnMut(&Path) -> anyhow::Result<()>,
    emit: &dyn Fn(LibraryEvent),
) -> usize {
    let offline = match library.offline_folders() {
        Ok(folders) => folders,
        Err(err) => {
            eprintln!("listing offline folders failed: {err:#}");
            return 0;
        }
    };
    memo.retain(|folder, _| offline.contains(folder));
    let mut scans = 0;
    for folder in offline {
        let listing = match root_listing(&folder) {
            Ok(listing) if folder.is_dir() => listing,
            _ => {
                memo.remove(&folder);
                continue;
            }
        };
        if memo.get(&folder) == Some(&listing) {
            continue;
        }
        scans += 1;
        if let Err(err) = library.rescan_folder(&folder, emit) {
            eprintln!("rescanning {} failed: {err:#}", folder.display());
        }
        if matches!(library.online(&scan::folder_key(&folder)), Ok(Some(true))) {
            memo.remove(&folder);
            if let Err(err) = watch(&folder) {
                eprintln!("{err:#}");
            }
        } else {
            memo.insert(folder, listing);
        }
    }
    scans
}

/// Starts watching `folders`; each change rescans only the affected folder after the
/// 2 s debounce and emits the library events. Folders that cannot be watched (offline
/// drives) are skipped here and picked up by a check every 60 s once they are readable.
pub fn start(app: AppHandle, folders: Vec<PathBuf>) {
    let handle = app.clone();
    let watcher = FolderWatcher::new(DEBOUNCE, move |folder| {
        let state = handle.state::<AppState>();
        // A folder removed from the library is ignored until it is unwatched.
        if !state.library.is_registered(&folder).unwrap_or(false) {
            return;
        }
        let emit = tauri_emitter(handle.clone());
        if let Err(err) = state.library.rescan_folder(&folder, &emit) {
            eprintln!("rescanning {} failed: {err:#}", folder.display());
        }
    });
    let watcher = match watcher {
        Ok(mut watcher) => {
            for folder in &folders {
                if let Err(err) = watcher.watch(folder) {
                    eprintln!("{err:#}");
                }
            }
            Some(watcher)
        }
        Err(err) => {
            eprintln!("{err:#}");
            None
        }
    };
    app.manage(WatchState(Mutex::new(watcher)));

    let handle = app.clone();
    std::thread::spawn(move || {
        let mut memo = OfflineMemo::new();
        loop {
            std::thread::sleep(OFFLINE_RECHECK);
            // A panic in one cycle is logged; the next cycle still runs.
            let cycle = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let state = handle.state::<AppState>();
                let watch_state = handle.state::<WatchState>();
                let mut watch = |folder: &Path| match watch_state.0.lock() {
                    Ok(mut guard) => match guard.as_mut() {
                        Some(watcher) => watcher.rewatch(folder),
                        None => Ok(()),
                    },
                    Err(_) => Err(poisoned()),
                };
                recheck_offline(
                    &state.library,
                    &mut memo,
                    &mut watch,
                    &tauri_emitter(handle.clone()),
                );
            }));
            if cycle.is_err() {
                eprintln!("the offline folder check panicked; continuing");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::library::scan::tests::copy_fixtures;
    use crate::library::tests::library;
    use crate::library::LibraryEvent;

    #[test]
    fn watcher_ignores_temp_files() {
        assert!(!is_relevant(Path::new("a.flac.tmp"), false));
        assert!(!is_relevant(Path::new("notes.txt"), false));
        assert!(!is_relevant(Path::new("~lock.song.flac#"), false));
        assert!(is_relevant(Path::new("song.FLAC"), false));
        assert!(is_relevant(Path::new("folder.jpg"), false));
        assert!(is_relevant(Path::new("Cover.PNG"), false));
        assert!(!is_relevant(Path::new("holiday.jpg"), false));
        assert!(is_relevant(Path::new("Album"), true));
    }

    #[test]
    fn deleted_folder_with_a_dot_is_relevant() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("Album 2.0");
        assert!(!gone.exists());
        assert!(event_is_relevant(&gone));
        assert!(event_is_relevant(&dir.path().join("Vol. 2")));
        assert!(!event_is_relevant(&dir.path().join("a.flac.tmp")));
        assert!(!event_is_relevant(&dir.path().join("movie.part")));
        assert!(!event_is_relevant(&dir.path().join("notes.txt")));
    }

    #[test]
    fn empty_mount_is_scanned_once_and_a_new_file_brings_it_back() {
        let (library, _covers) = library();
        let dir = copy_fixtures(&["corrupt.flac"]);
        library.add_folder(dir.path(), &|_| {}).unwrap();
        let mount = dir.path().to_path_buf();
        for entry in std::fs::read_dir(&mount).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                std::fs::remove_dir_all(path).unwrap();
            } else {
                std::fs::remove_file(path).unwrap();
            }
        }
        // The empty-mount guard marks it offline.
        library.rescan_folder(&mount, &|_| {}).unwrap();
        assert_eq!(library.offline_folders().unwrap(), vec![mount.clone()]);

        let gone = tempfile::tempdir().unwrap().path().join("unplugged");
        library
            .db
            .0
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO folders (path, online) VALUES (?1, 0)",
                [gone.to_string_lossy()],
            )
            .unwrap();

        let watched = std::cell::RefCell::new(Vec::new());
        let events = std::cell::RefCell::new(Vec::new());
        let mut memo = OfflineMemo::new();
        let cycle = |memo: &mut OfflineMemo| {
            recheck_offline(
                &library,
                memo,
                &mut |folder| {
                    watched.borrow_mut().push(folder.to_path_buf());
                    Ok(())
                },
                &|event| events.borrow_mut().push(event),
            )
        };
        let scans: usize = (0..3).map(|_| cycle(&mut memo)).sum();
        assert_eq!(scans, 1);
        assert!(watched.borrow().is_empty());
        assert!(
            events.borrow().is_empty(),
            "a re-failed guard emits nothing"
        );

        std::fs::copy(
            crate::library::tags::fixtures().join("tagged.mp3"),
            mount.join("tagged.mp3"),
        )
        .unwrap();
        assert_eq!(cycle(&mut memo), 1);
        assert_eq!(*watched.borrow(), vec![mount.clone()]);
        assert!(events.borrow().contains(&LibraryEvent::Changed));
        assert_eq!(library.offline_folders().unwrap(), vec![gone]);
        assert_eq!(cycle(&mut memo), 0);
    }

    #[test]
    fn root_for_picks_the_longest_containing_root() {
        let roots: Vec<WatchRoot> = ["/music", "/music/rock", "/other"]
            .iter()
            .map(|key| WatchRoot {
                key: PathBuf::from(key),
                canonical: PathBuf::from(key),
            })
            .collect();
        assert_eq!(
            root_for(Path::new("/music/rock/a/b.mp3"), &roots),
            Some(&roots[1].key)
        );
        assert_eq!(
            root_for(Path::new("/music/jazz.mp3"), &roots),
            Some(&roots[0].key)
        );
        assert_eq!(root_for(Path::new("/musicals/x.mp3"), &roots), None);
    }

    #[test]
    fn matches_events_reported_under_the_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        // The library knows the folder under an alias; events arrive under the real path.
        let roots = vec![WatchRoot {
            key: PathBuf::from("/alias/music"),
            canonical: canonical.clone(),
        }];
        std::fs::create_dir(dir.path().join("album")).unwrap();
        let existing = dir.path().join("album");
        let deleted = dir.path().join("album").join("gone.mp3");
        assert_eq!(root_for(&existing, &roots), Some(&roots[0].key));
        assert_eq!(root_for(&deleted, &roots), Some(&roots[0].key));
        assert_eq!(
            canonicalize_lossy(&deleted),
            canonical.join("album").join("gone.mp3")
        );
        let elsewhere = tempfile::tempdir().unwrap();
        assert_eq!(root_for(elsewhere.path(), &roots), None);
    }

    #[test]
    fn reports_the_changed_folder_once_after_debounce() {
        let watched = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let mut watcher = FolderWatcher::new(Duration::from_millis(300), move |folder| {
            tx.send(folder).unwrap();
        })
        .unwrap();
        watcher.watch(watched.path()).unwrap();
        watcher.watch(other.path()).unwrap();

        let nested = watched.path().join("album");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("a.mp3"), b"1").unwrap();
        std::fs::write(nested.join("b.mp3"), b"2").unwrap();

        let folder = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("change reported");
        assert_eq!(folder, watched.path());
        // Only the affected root is ever reported, never the untouched one.
        while let Ok(folder) = rx.recv_timeout(Duration::from_millis(900)) {
            assert_eq!(folder, watched.path());
        }

        watcher.unwatch(watched.path()).unwrap();
        std::fs::write(watched.path().join("c.mp3"), b"3").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(900)).is_err());
    }
}
