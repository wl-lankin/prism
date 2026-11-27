//! The Navidrome password, kept in the OS keychain (Windows Credential Manager, macOS
//! Keychain, the Secret Service on Linux) and nowhere else on disk. A Linux session without a
//! Secret Service (a kiosk without a desktop) keeps it in a file only the user can read
//! instead, see [`platform_store`].
//!
//! Entries are keyed by the app identifier, so a dev build with its own identifier never
//! reads or writes the password of the installed app.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Context;

use crate::secret::Secret;

/// The [`FileStore`] file in the data folder.
pub const FILE_STORE_NAME: &str = "navidrome-password.json";

/// The password store of this platform for the app `identifier`, whose data lives in
/// `data_dir`.
///
/// Windows and macOS: the OS keychain. Linux: the Secret Service (gnome-keyring, KWallet) when
/// the session answers on it; otherwise a [`FileStore`] in `data_dir`, readable by the user
/// only. The choice is made once per start.
pub fn platform_store(identifier: &str, data_dir: &Path) -> Arc<dyn CredentialStore> {
    let keyring = KeyringStore::new(identifier);
    #[cfg(target_os = "linux")]
    if !keyring.reachable() {
        eprintln!(
            "no Secret Service in this session; the Navidrome password lives in {FILE_STORE_NAME}, readable by this user only"
        );
        return Arc::new(FileStore::new(data_dir.join(FILE_STORE_NAME)));
    }
    let _ = data_dir;
    Arc::new(keyring)
}

/// Where the password of an account lives.
pub trait CredentialStore: Send + Sync {
    /// The password of `user` on `url`, `None` when none is stored.
    fn get(&self, url: &str, user: &str) -> anyhow::Result<Option<Secret>>;
    /// Stores (or replaces) the password of `user` on `url`.
    fn set(&self, url: &str, user: &str, password: &Secret) -> anyhow::Result<()>;
    /// Forgets the password of `user` on `url`; nothing stored is not an error.
    fn delete(&self, url: &str, user: &str) -> anyhow::Result<()>;
}

/// The keychain account of `user` on `url`.
pub fn account_key(url: &str, user: &str) -> String {
    format!("{user}@{url}")
}

/// The OS keychain: Credential Manager on Windows (`windows-native`), the Keychain on macOS
/// (`apple-native`), the Secret Service on Linux (`sync-secret-service`).
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    /// `identifier` is the Tauri app identifier (`de.wolfganglinz.prism` or a dev one).
    pub fn new(identifier: &str) -> KeyringStore {
        KeyringStore {
            service: format!("{identifier}.navidrome"),
        }
    }

    /// The keychain service every entry of this app is filed under.
    pub fn service(&self) -> &str {
        &self.service
    }

    fn entry(&self, url: &str, user: &str) -> anyhow::Result<keyring::Entry> {
        keyring::Entry::new(&self.service, &account_key(url, user))
            .map_err(|err| failed("opening the keychain entry failed", &err))
    }

    /// True when the keychain answers: looking up an entry nobody stores comes back empty
    /// instead of failing. A Linux session may have no Secret Service at all.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn reachable(&self) -> bool {
        self.get("prism:probe", "prism").is_ok()
    }
}

/// Passwords in a JSON file only the user can read (mode 0600), for a Linux session without a
/// Secret Service. Every change replaces the whole file through a temporary one, so a crash
/// never leaves half a file.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct FileStore {
    path: PathBuf,
    lock: Mutex<()>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl FileStore {
    pub fn new(path: PathBuf) -> FileStore {
        FileStore {
            path,
            lock: Mutex::new(()),
        }
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        match self.lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// The stored entries; no file is no entries. Errors name the kind only, never content.
    fn read(&self) -> anyhow::Result<HashMap<String, String>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("the password file is damaged")),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(err) => Err(anyhow::anyhow!(
                "the password file cannot be read: {}",
                err.kind()
            )),
        }
    }

    /// Replaces the file with `entries`, or removes it when there are none.
    fn write(&self, entries: &HashMap<String, String>) -> anyhow::Result<()> {
        if entries.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(err) => Err(anyhow::anyhow!(
                    "the password file cannot be removed: {}",
                    err.kind()
                )),
            };
        }
        let dir = self
            .path
            .parent()
            .context("the password file has no folder")?;
        std::fs::create_dir_all(dir).context("creating the data folder failed")?;
        let tmp = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(entries).context("encoding the password file failed")?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let written = options.open(&tmp).and_then(|mut file| {
            // `mode` only applies to a new file; a leftover temporary file is tightened too.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(&bytes)?;
            file.sync_all()
        });
        if let Err(err) = written.and_then(|()| std::fs::rename(&tmp, &self.path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(anyhow::anyhow!(
                "the password file cannot be written: {}",
                err.kind()
            ));
        }
        Ok(())
    }
}

impl CredentialStore for FileStore {
    fn get(&self, url: &str, user: &str) -> anyhow::Result<Option<Secret>> {
        let _guard = self.guard();
        let entries = self.read().context("reading the password failed")?;
        Ok(entries
            .get(&account_key(url, user))
            .cloned()
            .map(Secret::new))
    }

    fn set(&self, url: &str, user: &str, password: &Secret) -> anyhow::Result<()> {
        let _guard = self.guard();
        let mut entries = self.read().context("saving the password failed")?;
        entries.insert(account_key(url, user), password.expose().to_owned());
        self.write(&entries).context("saving the password failed")
    }

    fn delete(&self, url: &str, user: &str) -> anyhow::Result<()> {
        let _guard = self.guard();
        let mut entries = self.read().context("deleting the password failed")?;
        if entries.remove(&account_key(url, user)).is_none() {
            return Ok(());
        }
        self.write(&entries).context("deleting the password failed")
    }
}

/// The keychain refused a value as too long. Windows Credential Manager caps the entry's user
/// name (`<user>@<url>`, see [`account_key`]) at 513 characters. Find it with [`is_too_long`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLong;

impl std::fmt::Display for TooLong {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a value is too long for the keychain")
    }
}

impl std::error::Error for TooLong {}

/// True when the keychain refused a value as too long.
pub fn is_too_long(err: &anyhow::Error) -> bool {
    err.downcast_ref::<TooLong>().is_some()
}

/// A keyring error under `action`; `TooLong` stays typed for [`is_too_long`].
fn failed(action: &'static str, err: &keyring::Error) -> anyhow::Error {
    match err {
        keyring::Error::TooLong(_, _) => anyhow::Error::new(TooLong).context(action),
        _ => anyhow::anyhow!("{action}: {}", describe(err)),
    }
}

/// A keychain error as text. Keyring errors carry no password, but only the kind is named,
/// so nothing from the platform's message can carry one either.
fn describe(err: &keyring::Error) -> &'static str {
    match err {
        keyring::Error::PlatformFailure(_) => "the keychain failed",
        keyring::Error::NoStorageAccess(_) => "the keychain is not accessible",
        keyring::Error::NoEntry => "no entry",
        keyring::Error::BadEncoding(_) => "the stored password is not valid text",
        keyring::Error::TooLong(_, _) => "a value is too long for the keychain",
        keyring::Error::Invalid(_, _) => "the keychain refused a value",
        keyring::Error::Ambiguous(_) => "the keychain holds more than one matching entry",
        _ => "unknown keychain error",
    }
}

impl CredentialStore for KeyringStore {
    fn get(&self, url: &str, user: &str) -> anyhow::Result<Option<Secret>> {
        match self.entry(url, user)?.get_password() {
            Ok(password) => Ok(Some(Secret::new(password))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(failed("reading the password failed", &err)),
        }
    }

    fn set(&self, url: &str, user: &str, password: &Secret) -> anyhow::Result<()> {
        self.entry(url, user)?
            .set_password(password.expose())
            .map_err(|err| failed("saving the password failed", &err))
    }

    fn delete(&self, url: &str, user: &str) -> anyhow::Result<()> {
        match self.entry(url, user)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(failed("deleting the password failed", &err)),
        }
    }
}

/// An in-memory store for tests (of this and other modules).
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, Secret>>,
    /// Refuses every read and write with [`TooLong`], as Windows does for a long account.
    too_long: bool,
}

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }

    /// A store that refuses every read and write with [`TooLong`].
    pub fn too_long() -> MemoryStore {
        MemoryStore {
            too_long: true,
            ..MemoryStore::default()
        }
    }

    fn refuse(&self, action: &'static str) -> anyhow::Result<()> {
        if self.too_long {
            return Err(anyhow::Error::new(TooLong).context(action));
        }
        Ok(())
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Secret>> {
        match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl CredentialStore for MemoryStore {
    fn get(&self, url: &str, user: &str) -> anyhow::Result<Option<Secret>> {
        self.refuse("reading the password failed")?;
        Ok(self.entries().get(&account_key(url, user)).cloned())
    }

    fn set(&self, url: &str, user: &str, password: &Secret) -> anyhow::Result<()> {
        self.refuse("saving the password failed")?;
        self.entries()
            .insert(account_key(url, user), password.clone());
        Ok(())
    }

    fn delete(&self, url: &str, user: &str) -> anyhow::Result<()> {
        self.entries().remove(&account_key(url, user));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_service_uses_identifier() {
        // Pure: building the store touches no keychain.
        let store = KeyringStore::new("de.wolfganglinz.prism.dev1");
        assert_eq!(store.service(), "de.wolfganglinz.prism.dev1.navidrome");
    }

    #[test]
    fn account_key_joins_user_and_url() {
        assert_eq!(
            account_key("https://nas:4533", "demo"),
            "demo@https://nas:4533"
        );
    }

    #[test]
    fn too_long_stays_recognizable_under_its_context() {
        let store = MemoryStore::too_long();
        let err = store
            .set("http://127.0.0.1:4533", "demo", &Secret::new("demo-pass"))
            .unwrap_err();
        assert!(is_too_long(&err));
        assert_eq!(
            format!("{err:#}"),
            "saving the password failed: a value is too long for the keychain"
        );
        assert!(is_too_long(
            &store.get("http://127.0.0.1:4533", "demo").unwrap_err()
        ));
        assert!(!is_too_long(&anyhow::anyhow!("the keychain failed")));
        let other = failed(
            "saving the password failed",
            &keyring::Error::NoStorageAccess("locked".into()),
        );
        assert!(!is_too_long(&other));
        let long = failed(
            "saving the password failed",
            &keyring::Error::TooLong("username".into(), 513),
        );
        assert!(is_too_long(&long));
    }

    #[test]
    fn memory_store_round_trip() {
        let store = MemoryStore::new();
        let url = "http://127.0.0.1:4533";
        assert_eq!(store.get(url, "demo").unwrap(), None);
        store.set(url, "demo", &Secret::new("demo-pass")).unwrap();
        assert_eq!(
            store.get(url, "demo").unwrap(),
            Some(Secret::new("demo-pass"))
        );
        assert_eq!(
            store.get(url, "other").unwrap(),
            None,
            "keyed by user and url"
        );
        assert_eq!(store.get("http://127.0.0.1:1", "demo").unwrap(), None);
        store.set(url, "demo", &Secret::new("new-pass")).unwrap();
        assert_eq!(
            store.get(url, "demo").unwrap(),
            Some(Secret::new("new-pass"))
        );
        store.delete(url, "demo").unwrap();
        assert_eq!(store.get(url, "demo").unwrap(), None);
        store
            .delete(url, "demo")
            .expect("deleting nothing is a no-op");
    }

    fn file_store() -> (tempfile::TempDir, FileStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path().join("data").join(FILE_STORE_NAME));
        (dir, store)
    }

    #[test]
    fn file_store_keeps_and_forgets_passwords() {
        let (_dir, store) = file_store();
        let url = "http://127.0.0.1:4533";
        assert_eq!(store.get(url, "demo").unwrap(), None);
        store.set(url, "demo", &Secret::new("demo-pass")).unwrap();
        store.set(url, "other", &Secret::new("other-pass")).unwrap();
        assert_eq!(
            store.get(url, "demo").unwrap(),
            Some(Secret::new("demo-pass"))
        );
        store.set(url, "demo", &Secret::new("new-pass")).unwrap();
        assert_eq!(
            store.get(url, "demo").unwrap(),
            Some(Secret::new("new-pass"))
        );
        store.delete(url, "demo").unwrap();
        assert_eq!(store.get(url, "demo").unwrap(), None);
        assert_eq!(
            store.get(url, "other").unwrap(),
            Some(Secret::new("other-pass"))
        );
    }

    #[test]
    fn file_store_survives_a_restart() {
        let (_dir, store) = file_store();
        store
            .set("http://127.0.0.1:4533", "demo", &Secret::new("demo-pass"))
            .unwrap();
        let again = FileStore::new(store.path.clone());
        assert_eq!(
            again.get("http://127.0.0.1:4533", "demo").unwrap(),
            Some(Secret::new("demo-pass"))
        );
    }

    #[test]
    fn file_store_removes_the_file_with_the_last_password() {
        let (_dir, store) = file_store();
        store.delete("http://127.0.0.1:4533", "demo").unwrap();
        store
            .set("http://127.0.0.1:4533", "demo", &Secret::new("demo-pass"))
            .unwrap();
        assert!(store.path.is_file());
        store.delete("http://127.0.0.1:4533", "demo").unwrap();
        assert!(!store.path.exists());
        assert!(!store.path.with_extension("json.tmp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn file_store_is_readable_by_the_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = file_store();
        // A leftover temporary file from a crash, readable by everyone.
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        let tmp = store.path.with_extension("json.tmp");
        std::fs::write(&tmp, b"{}").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
        store
            .set("http://127.0.0.1:4533", "demo", &Secret::new("demo-pass"))
            .unwrap();
        let mode = std::fs::metadata(&store.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_damaged_file_store_names_no_content() {
        let (_dir, store) = file_store();
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(&store.path, b"{\"demo@x\": \"demo-pass\"").unwrap();
        let err = store.get("x", "demo").unwrap_err();
        let text = format!("{err:#}");
        assert_eq!(
            text,
            "reading the password failed: the password file is damaged"
        );
        assert!(!text.contains("demo-pass"));
    }
}
