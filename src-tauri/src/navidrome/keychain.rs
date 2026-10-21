//! The Navidrome password, kept in the OS keychain (Windows Credential Manager, macOS
//! Keychain) and nowhere else on disk.
//!
//! Entries are keyed by the app identifier, so a dev build with its own identifier never
//! reads or writes the password of the installed app.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::secret::Secret;

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
/// (`apple-native`).
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
            .map_err(|err| anyhow::anyhow!("opening the keychain entry failed: {}", describe(&err)))
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
            Err(err) => anyhow::bail!("reading the password failed: {}", describe(&err)),
        }
    }

    fn set(&self, url: &str, user: &str, password: &Secret) -> anyhow::Result<()> {
        self.entry(url, user)?
            .set_password(password.expose())
            .map_err(|err| anyhow::anyhow!("saving the password failed: {}", describe(&err)))
    }

    fn delete(&self, url: &str, user: &str) -> anyhow::Result<()> {
        match self.entry(url, user)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => anyhow::bail!("deleting the password failed: {}", describe(&err)),
        }
    }
}

/// An in-memory store for tests (of this and other modules).
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, Secret>>,
}

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
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
        Ok(self.entries().get(&account_key(url, user)).cloned())
    }

    fn set(&self, url: &str, user: &str, password: &Secret) -> anyhow::Result<()> {
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
}
