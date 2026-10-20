//! User settings, stored as `<appConfig>/settings.json`.
//!
//! Writes are atomic (a temp file renamed over the old one). A file that cannot be read as
//! settings is kept as `settings.json.bad` and the defaults are used instead.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    System,
    En,
    De,
}

/// Which ReplayGain values the engine applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplayGain {
    #[default]
    Off,
    Track,
    Album,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub language: Language,
    /// 0..=1
    pub volume: f32,
    /// A device name as `audio_devices` lists it; `None` is the system default.
    pub output_device: Option<String>,
    /// Open the next track early so it starts without a gap.
    pub gapless: bool,
    pub replay_gain: ReplayGain,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            language: Language::System,
            volume: 0.8,
            output_device: None,
            gapless: true,
            replay_gain: ReplayGain::Off,
        }
    }
}

impl Settings {
    /// Keeps values in range (a hand-edited file may hold anything).
    fn sanitized(mut self) -> Settings {
        self.volume = if self.volume.is_finite() {
            self.volume.clamp(0.0, 1.0)
        } else {
            Settings::default().volume
        };
        if self
            .output_device
            .as_deref()
            .is_some_and(|name| name.trim().is_empty())
        {
            self.output_device = None;
        }
        self
    }
}

/// A setting the player has to follow.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Volume(f32),
    OutputDevice(Option<String>),
    Gapless(bool),
    ReplayGain(ReplayGain),
}

/// The player-relevant differences from `before` to `after` (the language is the UI's).
pub fn changes(before: &Settings, after: &Settings) -> Vec<Change> {
    let mut changes = Vec::new();
    if after.volume != before.volume {
        changes.push(Change::Volume(after.volume));
    }
    if after.output_device != before.output_device {
        changes.push(Change::OutputDevice(after.output_device.clone()));
    }
    if after.gapless != before.gapless {
        changes.push(Change::Gapless(after.gapless));
    }
    if after.replay_gain != before.replay_gain {
        changes.push(Change::ReplayGain(after.replay_gain));
    }
    changes
}

/// The settings file and the values in it.
pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
    /// Set when an existing file could not be read and could not be moved aside either:
    /// saving would overwrite it, so changes are refused.
    blocked: Option<String>,
}

impl SettingsStore {
    /// Reads `path`. A missing file gives the defaults; an unreadable one is renamed to
    /// `<name>.bad` and gives the defaults too.
    pub fn load(path: &Path) -> SettingsStore {
        let mut blocked = None;
        let current = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(settings) => settings.sanitized(),
                Err(err) => {
                    eprintln!("the settings file is unreadable, using defaults: {err}");
                    let bad = sibling(path, ".bad");
                    if let Err(err) = std::fs::rename(path, &bad) {
                        eprintln!("keeping the unreadable settings failed: {err}");
                    }
                    Settings::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(err) => {
                // The file exists but cannot be read (locked, no permission): keep it.
                eprintln!("reading the settings failed, using defaults: {err}");
                if let Err(rename) = std::fs::rename(path, sibling(path, ".bad")) {
                    eprintln!("keeping the unreadable settings failed: {rename}");
                    blocked = Some(format!(
                        "{} could not be read ({err}), so it is not overwritten",
                        path.display()
                    ));
                }
                Settings::default()
            }
        };
        SettingsStore {
            path: path.to_path_buf(),
            current: Mutex::new(current),
            blocked,
        }
    }

    pub fn get(&self) -> Settings {
        match self.current.lock() {
            Ok(current) => current.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Merges `patch` (an object with some of the settings' camelCase keys) into the
    /// settings, saves them and returns the result. Unknown keys are ignored; a value of the
    /// wrong type is refused and nothing changes.
    pub fn set(&self, patch: Value) -> anyhow::Result<Settings> {
        self.update(patch, |_| {})
    }

    /// Like [`SettingsStore::set`], and calls `apply` with what changed while the settings
    /// are still locked: concurrent updates reach `apply` one at a time, in the order they
    /// were saved, and each change exactly once.
    pub fn update(&self, patch: Value, apply: impl FnOnce(&[Change])) -> anyhow::Result<Settings> {
        if let Some(reason) = &self.blocked {
            anyhow::bail!("{reason}");
        }
        let Value::Object(patch) = patch else {
            anyhow::bail!("settings must be changed with an object");
        };
        let mut current = match self.current.lock() {
            Ok(current) => current,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut merged = serde_json::to_value(&*current)?;
        if let Value::Object(fields) = &mut merged {
            for (key, value) in patch {
                if fields.contains_key(&key) {
                    fields.insert(key, value);
                }
            }
        }
        let next = serde_json::from_value::<Settings>(merged)
            .context("invalid setting")?
            .sanitized();
        save(&self.path, &next)?;
        let changed = changes(&current, &next);
        *current = next.clone();
        apply(&changed);
        Ok(next)
    }
}

/// `path` with `suffix` appended to its file name.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Writes to a temp file next to `path`, then renames it over `path`.
fn save(path: &Path, settings: &Settings) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let temp = sibling(path, ".tmp");
    let json = serde_json::to_vec_pretty(settings)?;
    std::fs::write(&temp, json).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("settings.json");

        let store = SettingsStore::load(&path);
        assert_eq!(
            store.get(),
            Settings::default(),
            "a missing file gives defaults"
        );
        assert_eq!(store.get().volume, 0.8);
        assert!(store.get().gapless);

        let saved = store
            .set(json!({ "volume": 0.5, "replayGain": "album", "outputDevice": "Speakers" }))
            .unwrap();
        assert_eq!(saved.volume, 0.5);
        assert_eq!(saved.replay_gain, ReplayGain::Album);
        assert_eq!(saved.output_device.as_deref(), Some("Speakers"));
        assert_eq!(
            saved.language,
            Language::System,
            "untouched keys keep their value"
        );
        assert!(
            !sibling(&path, ".tmp").exists(),
            "the temp file was renamed"
        );

        let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["replayGain"], "album");
        assert_eq!(raw["outputDevice"], "Speakers");

        let reloaded = SettingsStore::load(&path);
        assert_eq!(reloaded.get(), saved, "the patch was merged and saved");

        let cleared = reloaded
            .set(json!({ "outputDevice": null, "language": "de" }))
            .unwrap();
        assert_eq!(cleared.output_device, None);
        assert_eq!(cleared.language, Language::De);
    }

    #[test]
    fn concurrent_sets_apply_each_change_once_and_in_order() {
        use std::sync::{Arc, Mutex as StdMutex};
        for round in 0..50 {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(SettingsStore::load(&dir.path().join("settings.json")));
            let applied = Arc::new(StdMutex::new(Vec::<Change>::new()));
            let device = format!("USB DAC {round}");
            let threads: Vec<_> = [
                json!({ "volume": 0.3 }),
                json!({ "outputDevice": device }),
                json!({ "volume": 0.4 }),
            ]
            .into_iter()
            .map(|patch| {
                let store = Arc::clone(&store);
                let applied = Arc::clone(&applied);
                std::thread::spawn(move || {
                    store
                        .update(patch, |changes| {
                            applied.lock().unwrap().extend_from_slice(changes)
                        })
                        .unwrap();
                })
            })
            .collect();
            for thread in threads {
                thread.join().unwrap();
            }
            let applied = applied.lock().unwrap().clone();
            let devices: Vec<_> = applied
                .iter()
                .filter(|change| matches!(change, Change::OutputDevice(_)))
                .collect();
            assert_eq!(devices, vec![&Change::OutputDevice(Some(device.clone()))]);
            let saved = store.get();
            assert_eq!(saved.output_device.as_deref(), Some(device.as_str()));
            let last_volume = applied.iter().rev().find_map(|change| match change {
                Change::Volume(volume) => Some(*volume),
                _ => None,
            });
            assert_eq!(
                last_volume,
                Some(saved.volume),
                "the player ends on the saved volume"
            );
            let reloaded = SettingsStore::load(&dir.path().join("settings.json")).get();
            assert_eq!(reloaded, saved);
        }
    }

    #[test]
    fn changes_lists_only_what_differs() {
        let before = Settings::default();
        let after = Settings {
            gapless: false,
            replay_gain: ReplayGain::Track,
            ..Settings::default()
        };
        assert_eq!(
            changes(&before, &after),
            vec![
                Change::Gapless(false),
                Change::ReplayGain(ReplayGain::Track)
            ]
        );
        assert!(changes(&before, &before).is_empty());
    }

    #[test]
    fn invalid_patches_change_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(&path);
        assert!(store.set(json!({ "replayGain": "loud" })).is_err());
        assert!(store.set(json!({ "volume": "high" })).is_err());
        assert!(store.set(json!([1, 2])).is_err());
        assert_eq!(store.get(), Settings::default());
        assert!(!path.exists(), "nothing was written");

        let clamped = store
            .set(json!({ "volume": 3.0, "unknown": true }))
            .unwrap();
        assert_eq!(clamped.volume, 1.0);
    }

    #[test]
    fn a_corrupt_file_falls_back_to_defaults_and_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let store = SettingsStore::load(&path);
        assert_eq!(store.get(), Settings::default());
        let bad = dir.path().join("settings.json.bad");
        assert_eq!(std::fs::read(&bad).unwrap(), b"{ not json");
        assert!(!path.exists());
    }

    #[test]
    fn an_unreadable_file_is_moved_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        // A directory in its place cannot be read as a file.
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("marker"), b"keep").unwrap();
        let store = SettingsStore::load(&path);
        assert_eq!(store.get(), Settings::default());
        let bad = dir.path().join("settings.json.bad");
        assert_eq!(std::fs::read(bad.join("marker")).unwrap(), b"keep");
        store.set(json!({ "gapless": false })).unwrap();
        assert!(path.is_file());
    }

    #[test]
    fn missing_keys_take_their_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, br#"{ "gapless": false }"#).unwrap();
        let settings = SettingsStore::load(&path).get();
        assert!(!settings.gapless);
        assert_eq!(settings.volume, 0.8);
        assert_eq!(settings.replay_gain, ReplayGain::Off);
    }
}
