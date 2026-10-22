//! User settings, stored as `<appConfig>/settings.json`.
//!
//! Writes are atomic (a temp file renamed over the old one). A file that cannot be read as
//! settings is kept as `settings.json.bad` and the defaults are used instead.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::navidrome::client::StreamQuality;

/// Sent to every window with the full settings after each save.
pub const EVENT_CHANGED: &str = "settings://changed";

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

/// When tracks blend into each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Crossfade {
    Off,
    /// Only inside a Flow; any other queue plays without a crossfade.
    #[default]
    Flow,
    Always,
}

/// What happens at the end of a track and of the queue. Ignored in Flow, which never ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    /// The queue plays once.
    #[default]
    Off,
    /// After the last track the first one follows.
    All,
    /// The current track plays again at its end.
    One,
}

/// Where "Radio near you" looks.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum RadioLocation {
    /// Asked of an IP lookup, the first time the radio view needs it.
    #[default]
    Auto,
    /// Chosen by the user; no location service is ever called.
    #[serde(rename_all = "camelCase")]
    Manual {
        /// Two uppercase ASCII letters.
        country_code: String,
        region: Option<String>,
    },
}

impl RadioLocation {
    /// A manual place needs a two-letter country code (uppercased here); a blank region is
    /// none. Anything else is `Auto`.
    fn normalized(self) -> RadioLocation {
        match self {
            RadioLocation::Manual {
                country_code,
                region,
            } => {
                let code = country_code.trim();
                if code.len() != 2 || !code.bytes().all(|b| b.is_ascii_alphabetic()) {
                    return RadioLocation::Auto;
                }
                let region = region
                    .map(|region| region.trim().to_owned())
                    .filter(|region| !region.is_empty());
                RadioLocation::Manual {
                    country_code: code.to_ascii_uppercase(),
                    region,
                }
            }
            auto => auto,
        }
    }
}

/// The crossfade length range in milliseconds.
pub const CROSSFADE_MS_RANGE: std::ops::RangeInclusive<u32> = 1000..=8000;

/// How long the volume has to stay put before it is written to disk.
const VOLUME_WRITE_DELAY: Duration = Duration::from_millis(400);

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
    pub crossfade: Crossfade,
    /// 1000..=8000
    pub crossfade_ms: u32,
    /// Look up missing covers online.
    pub cover_lookup: bool,
    /// Keep the mini player above other windows.
    pub mini_on_top: bool,
    /// Global shortcut that shows the mini player.
    pub mini_shortcut: String,
    pub start_with_system: bool,
    pub repeat: RepeatMode,
    /// The upcoming part of the queue plays in random order.
    pub shuffle: bool,
    /// The connected Navidrome server (normalized). Only connect and disconnect write it
    /// ([`SettingsStore::set_navidrome_account`]); the password lives in the OS keychain.
    pub navidrome_url: Option<String>,
    /// The connected Navidrome user, written together with `navidrome_url`.
    pub navidrome_user: Option<String>,
    /// Read by the stream opener at every open, so it needs no [`Change`].
    #[serde(deserialize_with = "lenient_quality")]
    pub navidrome_quality: StreamQuality,
    /// Read by the radio service per request, so it needs no [`Change`].
    #[serde(deserialize_with = "lenient_radio_location")]
    pub radio_location: RadioLocation,
    /// Download updates in the background and install them on quit; off only announces them.
    pub auto_update: bool,
    /// Ask lrclib.net for lyrics. Read by the lyrics service at every call, so it needs no
    /// [`Change`].
    pub lyrics_lookup: bool,
}

/// The patch keys only connect and disconnect may write.
const ACCOUNT_KEYS: [&str; 2] = ["navidromeUrl", "navidromeUser"];

/// Reads a stream quality; an unknown value (a later version's, a hand edit) is the default
/// instead of an error, so the rest of the file is kept.
fn lenient_quality<'de, D: Deserializer<'de>>(deserializer: D) -> Result<StreamQuality, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(StreamQuality::deserialize(value).unwrap_or_default())
}

/// Reads the radio location; a broken value is `Auto` instead of an error, so the rest of the
/// file is kept.
fn lenient_radio_location<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<RadioLocation, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(RadioLocation::deserialize(value)
        .map(RadioLocation::normalized)
        .unwrap_or_default())
}

/// Removes the Navidrome account from a settings patch: the UI connects and disconnects
/// through their own commands, which check the server and keep the keychain in step.
pub fn strip_account_keys(patch: &mut Value) {
    if let Value::Object(fields) = patch {
        for key in ACCOUNT_KEYS {
            fields.remove(key);
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            language: Language::System,
            volume: 0.8,
            output_device: None,
            gapless: true,
            replay_gain: ReplayGain::Off,
            crossfade: Crossfade::Flow,
            crossfade_ms: 4000,
            cover_lookup: true,
            mini_on_top: false,
            mini_shortcut: "CommandOrControl+Alt+P".to_string(),
            start_with_system: false,
            repeat: RepeatMode::Off,
            shuffle: false,
            navidrome_url: None,
            navidrome_user: None,
            navidrome_quality: StreamQuality::Original,
            radio_location: RadioLocation::Auto,
            auto_update: true,
            lyrics_lookup: true,
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
        self.crossfade_ms = self
            .crossfade_ms
            .clamp(*CROSSFADE_MS_RANGE.start(), *CROSSFADE_MS_RANGE.end());
        if self.mini_shortcut.trim().is_empty() {
            self.mini_shortcut = Settings::default().mini_shortcut;
        }
        if self
            .output_device
            .as_deref()
            .is_some_and(|name| name.trim().is_empty())
        {
            self.output_device = None;
        }
        // An account is a url and a user; half of one (a hand edit) is none.
        let blank = |value: &Option<String>| value.as_deref().is_none_or(|v| v.trim().is_empty());
        if blank(&self.navidrome_url) || blank(&self.navidrome_user) {
            self.navidrome_url = None;
            self.navidrome_user = None;
        }
        self
    }
}

/// A setting the player or the shell (tray, windows, shortcuts) has to follow.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Volume(f32),
    OutputDevice(Option<String>),
    Gapless(bool),
    ReplayGain(ReplayGain),
    Crossfade(Crossfade, u32),
    CoverLookup(bool),
    MiniOnTop(bool),
    MiniShortcut(String),
    StartWithSystem(bool),
    Repeat(RepeatMode),
    Shuffle(bool),
    AutoUpdate(bool),
}

/// The differences from `before` to `after` that something has to follow (the language is
/// the UI's).
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
    if after.crossfade != before.crossfade || after.crossfade_ms != before.crossfade_ms {
        changes.push(Change::Crossfade(after.crossfade, after.crossfade_ms));
    }
    if after.cover_lookup != before.cover_lookup {
        changes.push(Change::CoverLookup(after.cover_lookup));
    }
    if after.mini_on_top != before.mini_on_top {
        changes.push(Change::MiniOnTop(after.mini_on_top));
    }
    if after.mini_shortcut != before.mini_shortcut {
        changes.push(Change::MiniShortcut(after.mini_shortcut.clone()));
    }
    if after.start_with_system != before.start_with_system {
        changes.push(Change::StartWithSystem(after.start_with_system));
    }
    if after.repeat != before.repeat {
        changes.push(Change::Repeat(after.repeat));
    }
    if after.shuffle != before.shuffle {
        changes.push(Change::Shuffle(after.shuffle));
    }
    if after.auto_update != before.auto_update {
        changes.push(Change::AutoUpdate(after.auto_update));
    }
    changes
}

/// The settings file and the values in it.
pub struct SettingsStore {
    path: PathBuf,
    current: Arc<Mutex<Settings>>,
    /// Set when an existing file could not be read and could not be moved aside either:
    /// saving would overwrite it, so changes are refused.
    blocked: Option<String>,
    volume_write: Arc<Mutex<VolumeWrite>>,
}

/// A remembered volume that is not on disk yet.
#[derive(Default)]
struct VolumeWrite {
    /// When the helper thread writes it; `None` when nothing is waiting.
    due: Option<Instant>,
    thread_running: bool,
}

/// Locks, also after another thread panicked while holding the lock.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
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
            current: Arc::new(Mutex::new(current)),
            blocked,
            volume_write: Arc::new(Mutex::new(VolumeWrite::default())),
        }
    }

    pub fn get(&self) -> Settings {
        match self.current.lock() {
            Ok(current) => current.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Merges `patch` (an object with some of the settings' camelCase keys) into the
    /// settings, saves them and returns the result. Unknown keys and the Navidrome account
    /// are ignored; a value of the wrong type is refused and nothing changes.
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
        let mut patch = patch;
        strip_account_keys(&mut patch);
        let Value::Object(patch) = patch else {
            anyhow::bail!("settings must be changed with an object");
        };
        // The file reads an unknown quality leniently; a patch with one is refused.
        if let Some(quality) = patch.get("navidromeQuality") {
            StreamQuality::deserialize(quality).context("invalid setting")?;
        }
        // Same for the radio location: the file falls back to Auto, a patch is refused.
        if let Some(location) = patch.get("radioLocation") {
            let valid = RadioLocation::deserialize(location).map(RadioLocation::normalized);
            anyhow::ensure!(
                valid.is_ok_and(|valid| valid != RadioLocation::Auto || location["mode"] == "auto"),
                "invalid setting"
            );
        }
        let mut current = lock(&self.current);
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
        // The file now holds everything, a remembered volume included.
        lock(&self.volume_write).due = None;
        let changed = changes(&current, &next);
        *current = next.clone();
        apply(&changed);
        Ok(next)
    }

    /// Saves the connected Navidrome account (`(url, user)`, the url normalized) or, with
    /// `None`, forgets it. Only connect and disconnect call this; the password goes to the
    /// keychain, never here. Nothing the player follows changes, so there is no [`Change`].
    pub fn set_navidrome_account(
        &self,
        account: Option<(String, String)>,
    ) -> anyhow::Result<Settings> {
        if let Some(reason) = &self.blocked {
            anyhow::bail!("{reason}");
        }
        let mut current = lock(&self.current);
        let mut next = current.clone();
        (next.navidrome_url, next.navidrome_user) = match account {
            Some((url, user)) => (Some(url), Some(user)),
            None => (None, None),
        };
        let next = next.sanitized();
        save(&self.path, &next)?;
        // The file now holds everything, a remembered volume included.
        lock(&self.volume_write).due = None;
        *current = next.clone();
        Ok(next)
    }

    /// Keeps `v` as the volume (clamped) and writes it to disk once it has stayed put for
    /// 400 ms, on a helper thread (a slider drag sends many values). Call
    /// [`SettingsStore::flush`] before the process ends to write a volume that is still
    /// waiting.
    pub fn remember_volume(&self, v: f32) {
        if self.blocked.is_some() || !v.is_finite() {
            return;
        }
        let volume = v.clamp(0.0, 1.0);
        let mut current = lock(&self.current);
        let mut pending = lock(&self.volume_write);
        if current.volume == volume && pending.due.is_none() {
            return;
        }
        current.volume = volume;
        pending.due = Some(Instant::now() + VOLUME_WRITE_DELAY);
        if pending.thread_running {
            return;
        }
        pending.thread_running = true;
        let path = self.path.clone();
        let shared = Arc::clone(&self.current);
        let volume_write = Arc::clone(&self.volume_write);
        let spawned = std::thread::Builder::new()
            .name("prism-settings".into())
            .spawn(move || write_when_due(&path, &shared, &volume_write));
        if let Err(err) = spawned {
            eprintln!("starting the settings writer failed: {err}");
            pending.thread_running = false;
        }
    }

    /// Writes a remembered volume that is still waiting, now.
    pub fn flush(&self) {
        write_pending(&self.path, &self.current, &self.volume_write);
    }
}

/// Saves the settings if a remembered volume is waiting. Takes the settings lock first, like
/// `update`, so the two never interleave.
fn write_pending(path: &Path, current: &Mutex<Settings>, volume_write: &Mutex<VolumeWrite>) {
    let current = lock(current);
    let mut pending = lock(volume_write);
    if pending.due.take().is_none() {
        return;
    }
    if let Err(err) = save(path, &current) {
        eprintln!("saving the volume failed: {err:#}");
    }
}

/// The helper thread: waits until the remembered volume is due, writes it, and ends once
/// nothing is waiting.
fn write_when_due(path: &Path, current: &Mutex<Settings>, volume_write: &Mutex<VolumeWrite>) {
    loop {
        let wait = {
            let mut pending = lock(volume_write);
            match pending.due {
                None => {
                    pending.thread_running = false;
                    return;
                }
                Some(due) => due.checked_duration_since(Instant::now()),
            }
        };
        match wait {
            Some(wait) => std::thread::sleep(wait),
            None => write_pending(path, current, volume_write),
        }
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
        let defaults = store.get();
        assert_eq!(defaults.crossfade, Crossfade::Flow);
        assert_eq!(defaults.crossfade_ms, 4000);
        assert!(defaults.cover_lookup);
        assert!(!defaults.mini_on_top);
        assert_eq!(defaults.mini_shortcut, "CommandOrControl+Alt+P");
        assert!(!defaults.start_with_system);

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
    fn crossfade_ms_is_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::load(&dir.path().join("settings.json"));
        let ms = |value: u32| {
            store
                .set(json!({ "crossfadeMs": value }))
                .unwrap()
                .crossfade_ms
        };
        assert_eq!(ms(10), 1000);
        assert_eq!(ms(60000), 8000);
        assert_eq!(ms(2500), 2500);
    }

    #[test]
    fn old_file_without_new_keys_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            br#"{ "language": "de", "volume": 0.3, "outputDevice": "USB DAC", "gapless": false, "replayGain": "track" }"#,
        )
        .unwrap();
        let settings = SettingsStore::load(&path).get();
        assert_eq!(settings.language, Language::De);
        assert_eq!(settings.volume, 0.3);
        assert_eq!(settings.output_device.as_deref(), Some("USB DAC"));
        assert!(!settings.gapless);
        assert_eq!(settings.replay_gain, ReplayGain::Track);
        let defaults = Settings::default();
        assert_eq!(settings.crossfade, defaults.crossfade);
        assert_eq!(settings.crossfade_ms, defaults.crossfade_ms);
        assert_eq!(settings.cover_lookup, defaults.cover_lookup);
        assert_eq!(settings.mini_on_top, defaults.mini_on_top);
        assert_eq!(settings.mini_shortcut, defaults.mini_shortcut);
        assert_eq!(settings.start_with_system, defaults.start_with_system);
        assert!(settings.auto_update, "updates install by default");
    }

    #[test]
    fn lyrics_lookup_is_on_by_default_and_saved_as_camel_case() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(&path);
        assert!(store.get().lyrics_lookup);
        let saved = store.set(json!({ "lyricsLookup": false })).unwrap();
        assert!(!saved.lyrics_lookup);
        assert!(!store.get().lyrics_lookup);
        assert_eq!(
            serde_json::to_value(store.get()).unwrap()["lyricsLookup"],
            json!(false)
        );
        store.flush();
        assert!(!SettingsStore::load(&path).get().lyrics_lookup);
    }

    #[test]
    fn auto_update_is_saved_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(&path);
        assert!(store.get().auto_update);
        let mut seen = Vec::new();
        store
            .update(json!({ "autoUpdate": false }), |changes| {
                seen.extend_from_slice(changes)
            })
            .unwrap();
        assert_eq!(seen, vec![Change::AutoUpdate(false)]);
        let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["autoUpdate"], false);
        assert!(!SettingsStore::load(&path).get().auto_update);
    }

    #[test]
    fn remembered_volume_is_flushed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(&path);
        store.remember_volume(0.3);
        assert_eq!(store.get().volume, 0.3, "kept in memory at once");
        store.flush();
        assert_eq!(SettingsStore::load(&path).get().volume, 0.3);
    }

    #[test]
    fn remembered_volume_is_written_after_the_delay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(&path);
        store.remember_volume(0.2);
        store.remember_volume(0.6);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(SettingsStore::load(&path).get().volume, 0.6);
    }

    #[test]
    fn changes_lists_the_shell_settings() {
        let before = Settings::default();
        let after = Settings {
            crossfade_ms: 2000,
            cover_lookup: false,
            mini_on_top: true,
            mini_shortcut: "Ctrl+Shift+M".into(),
            start_with_system: true,
            ..Settings::default()
        };
        assert_eq!(
            changes(&before, &after),
            vec![
                Change::Crossfade(Crossfade::Flow, 2000),
                Change::CoverLookup(false),
                Change::MiniOnTop(true),
                Change::MiniShortcut("Ctrl+Shift+M".into()),
                Change::StartWithSystem(true),
            ]
        );
    }

    #[test]
    fn repeat_and_shuffle_are_saved_and_listed_as_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(&path);
        let defaults = store.get();
        assert_eq!(defaults.repeat, RepeatMode::Off);
        assert!(!defaults.shuffle);

        let mut applied = Vec::new();
        let saved = store
            .update(json!({ "repeat": "all", "shuffle": true }), |changes| {
                applied.extend_from_slice(changes)
            })
            .unwrap();
        assert_eq!(saved.repeat, RepeatMode::All);
        assert!(saved.shuffle);
        assert_eq!(
            applied,
            vec![Change::Repeat(RepeatMode::All), Change::Shuffle(true)]
        );
        let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["repeat"], "all");
        assert_eq!(raw["shuffle"], true);
        let reloaded = SettingsStore::load(&path).get();
        assert_eq!(reloaded.repeat, RepeatMode::All);
        assert!(reloaded.shuffle);
        assert_eq!(
            store.set(json!({ "repeat": "one" })).unwrap().repeat,
            RepeatMode::One
        );
        assert!(store.set(json!({ "repeat": "twice" })).is_err());
    }

    #[test]
    fn settings_navidrome_defaults() {
        let defaults = Settings::default();
        assert_eq!(defaults.navidrome_url, None);
        assert_eq!(defaults.navidrome_user, None);
        assert_eq!(defaults.navidrome_quality, StreamQuality::Original);
        let raw = serde_json::to_value(&defaults).unwrap();
        assert_eq!(raw["navidromeUrl"], Value::Null);
        assert_eq!(raw["navidromeUser"], Value::Null);
        assert_eq!(raw["navidromeQuality"], "original");
    }

    #[test]
    fn unknown_quality_keeps_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            br#"{ "navidromeQuality": "999", "gapless": false, "navidromeUrl": "http://127.0.0.1:4533", "navidromeUser": "demo" }"#,
        )
        .unwrap();
        let settings = SettingsStore::load(&path).get();
        assert!(!settings.gapless, "the rest of the file is kept");
        assert_eq!(settings.navidrome_user.as_deref(), Some("demo"));
        assert_eq!(settings.navidrome_quality, StreamQuality::Original);
        assert!(
            !sibling(&path, ".bad").exists(),
            "the file was not moved aside"
        );
        assert!(path.exists());
    }

    #[test]
    fn settings_radio_location_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(&path);
        assert_eq!(store.get().radio_location, RadioLocation::Auto);
        let saved = store
            .update(
                json!({ "radioLocation": { "mode": "manual", "countryCode": "de", "region": " Testland " } }),
                |changes| assert!(changes.is_empty(), "the service reads it per request"),
            )
            .unwrap();
        let manual = RadioLocation::Manual {
            country_code: "DE".into(),
            region: Some("Testland".into()),
        };
        assert_eq!(saved.radio_location, manual);
        let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            raw["radioLocation"],
            json!({ "mode": "manual", "countryCode": "DE", "region": "Testland" })
        );
        assert_eq!(SettingsStore::load(&path).get().radio_location, manual);
        let saved = store
            .set(json!({ "radioLocation": { "mode": "auto" } }))
            .unwrap();
        assert_eq!(saved.radio_location, RadioLocation::Auto);
        assert!(store
            .set(json!({ "radioLocation": { "mode": "manual", "countryCode": "Deutschland" } }))
            .is_err());
        assert!(store.set(json!({ "radioLocation": 7 })).is_err());
    }

    #[test]
    fn broken_radio_location_falls_back_to_auto() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for broken in [
            r#"{"mode":"manual","countryCode":"Deutschland"}"#,
            r#"{"mode":"manual"}"#,
            r#"{"mode":"satellite"}"#,
            r#""manual""#,
            "42",
        ] {
            std::fs::write(
                &path,
                format!(r#"{{ "radioLocation": {broken}, "gapless": false }}"#),
            )
            .unwrap();
            let settings = SettingsStore::load(&path).get();
            assert_eq!(settings.radio_location, RadioLocation::Auto, "{broken}");
            assert!(!settings.gapless, "the rest of the file is kept: {broken}");
            assert!(!sibling(&path, ".bad").exists());
        }
    }

    #[test]
    fn known_quality_values_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, br#"{ "navidromeQuality": "192" }"#).unwrap();
        assert_eq!(
            SettingsStore::load(&path).get().navidrome_quality,
            StreamQuality::Kbps192
        );
    }

    #[test]
    fn quality_is_set_by_patch_and_an_invalid_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::load(&dir.path().join("settings.json"));
        let mut applied = Vec::new();
        let saved = store
            .update(json!({ "navidromeQuality": "128" }), |changes| {
                applied.extend_from_slice(changes)
            })
            .unwrap();
        assert_eq!(saved.navidrome_quality, StreamQuality::Kbps128);
        assert!(
            applied.is_empty(),
            "the opener reads the quality at each open"
        );
        assert!(store.set(json!({ "navidromeQuality": "999" })).is_err());
        assert_eq!(store.get().navidrome_quality, StreamQuality::Kbps128);
    }

    #[test]
    fn patch_cannot_set_the_account() {
        let mut patch = json!({
            "navidromeUrl": "http://127.0.0.1:1",
            "navidromeUser": "demo",
            "gapless": false,
        });
        strip_account_keys(&mut patch);
        assert_eq!(patch, json!({ "gapless": false }));

        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::load(&dir.path().join("settings.json"));
        let saved = store
            .set(json!({ "navidromeUrl": "http://127.0.0.1:1", "navidromeUser": "demo" }))
            .unwrap();
        assert_eq!(saved.navidrome_url, None);
        assert_eq!(saved.navidrome_user, None);
    }

    #[test]
    fn set_navidrome_account_saves_url_and_user_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(&path);
        let saved = store
            .set_navidrome_account(Some(("http://127.0.0.1:4533".into(), "demo".into())))
            .unwrap();
        assert_eq!(
            saved.navidrome_url.as_deref(),
            Some("http://127.0.0.1:4533")
        );
        assert_eq!(saved.navidrome_user.as_deref(), Some("demo"));
        assert_eq!(store.get(), saved);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.to_lowercase().contains("password"));
        assert!(!text.contains("demo-pass"));
        let raw: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(raw["navidromeUrl"], "http://127.0.0.1:4533");
        assert_eq!(raw["navidromeUser"], "demo");
        assert_eq!(SettingsStore::load(&path).get(), saved);

        let cleared = store.set_navidrome_account(None).unwrap();
        assert_eq!(cleared.navidrome_url, None);
        assert_eq!(cleared.navidrome_user, None);
        assert_eq!(SettingsStore::load(&path).get().navidrome_url, None);
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
