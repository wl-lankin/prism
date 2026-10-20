use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use tauri::{AppHandle, Manager};

use super::tauri_emitter;
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

/// Starts watching `folders`; each change rescans only the affected folder after the
/// 2 s debounce and emits the library events. Folders that cannot be watched (offline
/// drives) are skipped.
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
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

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
