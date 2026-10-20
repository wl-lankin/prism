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

/// The library folder that contains `path`: the longest matching root.
pub fn root_for<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
}

/// Watches library folders recursively and reports, per debounce window, each library
/// folder that saw a change.
pub struct FolderWatcher {
    debouncer: Debouncer<RecommendedWatcher>,
    roots: Arc<Mutex<Vec<PathBuf>>>,
}

impl FolderWatcher {
    pub fn new(
        debounce: Duration,
        on_change: impl Fn(PathBuf) + Send + 'static,
    ) -> anyhow::Result<FolderWatcher> {
        let roots: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::clone(&roots);
        let debouncer = new_debouncer(debounce, move |result: DebounceEventResult| {
            let Ok(events) = result else {
                return;
            };
            let roots = match shared.lock() {
                Ok(roots) => roots.clone(),
                Err(_) => return,
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
        if !roots.iter().any(|root| root == folder) {
            roots.push(folder.to_path_buf());
        }
        Ok(())
    }

    pub fn unwatch(&mut self, folder: &Path) -> anyhow::Result<()> {
        self.roots
            .lock()
            .map_err(|_| poisoned())?
            .retain(|root| root != folder);
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
        let roots = vec![
            PathBuf::from("/music"),
            PathBuf::from("/music/rock"),
            PathBuf::from("/other"),
        ];
        assert_eq!(
            root_for(Path::new("/music/rock/a/b.mp3"), &roots),
            Some(&roots[1])
        );
        assert_eq!(
            root_for(Path::new("/music/jazz.mp3"), &roots),
            Some(&roots[0])
        );
        assert_eq!(root_for(Path::new("/musicals/x.mp3"), &roots), None);
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
