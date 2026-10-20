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

/// The settings file and the values in it.
pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsStore {
    /// Reads `path`. A missing file gives the defaults; an unreadable one is renamed to
    /// `<name>.bad` and gives the defaults too.
    pub fn load(path: &Path) -> SettingsStore {
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
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("reading the settings failed, using defaults: {err}");
                }
                Settings::default()
            }
        };
        SettingsStore {
            path: path.to_path_buf(),
            current: Mutex::new(current),
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
        *current = next.clone();
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
