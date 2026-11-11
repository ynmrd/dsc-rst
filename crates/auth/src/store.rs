//! Secret storage. Tokens and client secrets live only here, never in the accounts JSON file.

use std::collections::HashMap;
use std::fmt;

use parking_lot::Mutex;

use crate::AuthError;

/// Key/value storage for secrets. Implementations must be safe to call from any thread.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, AuthError>;
    fn set(&self, key: &str, value: &str) -> Result<(), AuthError>;
    /// Deleting a key that does not exist is not an error.
    fn delete(&self, key: &str) -> Result<(), AuthError>;
}

/// Operating-system credential store (Windows Credential Manager, macOS Keychain, Linux keyutils).
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, AuthError> {
        Ok(keyring::Entry::new(&self.service, key)?)
    }
}

impl fmt::Debug for KeyringStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyringStore")
            .field("service", &self.service)
            .finish()
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, AuthError> {
        match self.entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), AuthError> {
        Ok(self.entry(key)?.set_password(value)?)
    }

    fn delete(&self, key: &str) -> Result<(), AuthError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// In-process store for tests and for running without an OS credential store.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sorted keys currently stored (values are never exposed this way).
    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.0.lock().keys().cloned().collect();
        keys.sort();
        keys
    }
}

impl fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStore")
            .field("entries", &self.0.lock().len())
            .finish()
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>, AuthError> {
        Ok(self.0.lock().get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), AuthError> {
        self.0.lock().insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), AuthError> {
        self.0.lock().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip_and_idempotent_delete() {
        let s = MemoryStore::new();
        assert_eq!(s.get("a").unwrap(), None);
        s.set("a", "1").unwrap();
        s.set("a", "2").unwrap();
        assert_eq!(s.get("a").unwrap().as_deref(), Some("2"));
        s.delete("a").unwrap();
        s.delete("a").unwrap();
        assert_eq!(s.get("a").unwrap(), None);
        assert!(s.keys().is_empty());
    }

    /// Uses keyring's mock backend (no OS keyring is touched). Mock entries do not share state,
    /// so this covers the "missing entry" mapping rather than a full roundtrip.
    #[test]
    fn keyring_store_treats_missing_entries_as_absent() {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let s = KeyringStore::new("DiscordNativeTest");
        assert_eq!(s.get("bot:missing").unwrap(), None);
        s.delete("bot:missing").unwrap();
        s.set("bot:missing", "value").unwrap();
        assert!(!format!("{s:?}").contains("value"));
    }

    #[test]
    fn memory_store_debug_hides_values() {
        let s = MemoryStore::new();
        s.set("bot:1", "super-secret-value").unwrap();
        assert!(!format!("{s:?}").contains("super-secret-value"));
    }
}
