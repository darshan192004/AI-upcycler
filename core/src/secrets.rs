//! OS-keychain-backed secret storage.
//!
//! Provider credentials live only in the native credential store. This module
//! never writes a secret value to SQLite, logs, telemetry, or API responses.

use crate::providers::ProviderId;
use keyring::Entry;
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::fmt;
#[cfg(test)]
use std::sync::Mutex;

const SERVICE: &str = "ai-upcycler";
const NO_OS_SECRET_STORE_ERROR: &str = "no OS secret store available";
const ENTRY_ERROR: &str = "OS secret store could not open the entry";
const WRITE_ERROR: &str = "OS secret store rejected the write";
const DELETE_ERROR: &str = "OS secret store rejected the delete";
#[cfg(test)]
const MEMORY_LOCK_ERROR: &str = "in-memory secret store is unavailable";

/// Identifies one credential without exposing its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretScope {
    Provider(ProviderId),
    /// Reserved for sub-project B's bearer token; no LAN listener exists here.
    LanToken,
}

impl SecretScope {
    /// Keychain account name. Stable so credentials survive restarts.
    fn key(self) -> String {
        match self {
            Self::Provider(provider) => format!("provider:{}", provider.as_str()),
            Self::LanToken => "lan-token".to_string(),
        }
    }
}

/// Reads and writes secrets without ever exposing them in `Debug` output.
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self, scope: SecretScope) -> Option<String>;
    fn set(&self, scope: SecretScope, value: &str) -> Result<(), String>;
    fn delete(&self, scope: SecretScope) -> Result<(), String>;
}

/// Production store backed by the platform credential manager.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringSecretStore;

impl KeyringSecretStore {
    fn entry(scope: SecretScope) -> Result<Entry, String> {
        let key = scope.key();
        Entry::new(SERVICE, &key).map_err(|_| ENTRY_ERROR.to_string())
    }
}

impl SecretStore for KeyringSecretStore {
    fn get(&self, scope: SecretScope) -> Option<String> {
        let entry = match Self::entry(scope) {
            Ok(entry) => entry,
            Err(_) => {
                tracing::debug!("keyring entry could not be opened");
                return None;
            }
        };
        match entry.get_password() {
            Ok(value) => Some(value),
            Err(keyring::Error::NoEntry) => None,
            Err(_) => {
                // Never format the error: some platform errors can carry bytes
                // from the stored value. Report only that a lookup failed.
                tracing::debug!("keyring lookup failed without a value");
                None
            }
        }
    }

    fn set(&self, scope: SecretScope, value: &str) -> Result<(), String> {
        Self::entry(scope)?
            .set_password(value)
            .map_err(|_| WRITE_ERROR.to_string())
    }

    fn delete(&self, scope: SecretScope) -> Result<(), String> {
        match Self::entry(scope)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(DELETE_ERROR.to_string()),
        }
    }
}

/// Fallback used when the host has no native credential store. It never holds
/// a value and gives the same explicit error for every write or delete.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnconfiguredSecretStore;

impl SecretStore for UnconfiguredSecretStore {
    fn get(&self, _scope: SecretScope) -> Option<String> {
        None
    }

    fn set(&self, _scope: SecretScope, _value: &str) -> Result<(), String> {
        Err(NO_OS_SECRET_STORE_ERROR.to_string())
    }

    fn delete(&self, _scope: SecretScope) -> Result<(), String> {
        Err(NO_OS_SECRET_STORE_ERROR.to_string())
    }
}

/// In-memory stand-in used only by unit tests. `Debug` reports the number of
/// entries, never the values.
#[cfg(test)]
#[derive(Default)]
struct MemorySecretStore {
    values: Mutex<HashMap<String, String>>,
}

#[cfg(test)]
impl fmt::Debug for MemorySecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let entries = match self.values.lock() {
            Ok(values) => values.len(),
            Err(_) => 0,
        };
        f.debug_struct("MemorySecretStore")
            .field("entries", &entries)
            .finish()
    }
}

#[cfg(test)]
impl SecretStore for MemorySecretStore {
    fn get(&self, scope: SecretScope) -> Option<String> {
        self.values.lock().ok()?.get(&scope.key()).cloned()
    }

    fn set(&self, scope: SecretScope, value: &str) -> Result<(), String> {
        let mut values = self
            .values
            .lock()
            .map_err(|_| MEMORY_LOCK_ERROR.to_string())?;
        values.insert(scope.key(), value.to_string());
        Ok(())
    }

    fn delete(&self, scope: SecretScope) -> Result<(), String> {
        let mut values = self
            .values
            .lock()
            .map_err(|_| MEMORY_LOCK_ERROR.to_string())?;
        values.remove(&scope.key());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYNTHETIC_KEY: &str = "sk-test-do-not-leak";

    fn provider_scope() -> SecretScope {
        SecretScope::Provider(ProviderId::OpenAI)
    }

    /// Exercise the injected `SecretStore` contract without touching a native
    /// credential store.
    fn assert_round_trip(store: &dyn SecretStore) {
        let scope = provider_scope();
        assert_eq!(store.get(scope), None);
        store
            .set(scope, SYNTHETIC_KEY)
            .expect("injected store accepts the write");
        assert_eq!(store.get(scope).as_deref(), Some(SYNTHETIC_KEY));
        store
            .delete(scope)
            .expect("injected store accepts the delete");
        assert_eq!(store.get(scope), None);
    }

    #[test]
    fn memory_store_round_trips_set_get_delete() {
        assert_round_trip(&MemorySecretStore::default());
    }

    #[test]
    fn missing_secret_returns_none() {
        assert_eq!(MemorySecretStore::default().get(provider_scope()), None);
        assert_eq!(UnconfiguredSecretStore.get(provider_scope()), None);
    }

    #[test]
    fn unknown_provider_id_is_rejected() {
        assert_eq!(ProviderId::parse(""), None);
        assert_eq!(ProviderId::parse("  "), None);
        assert_eq!(ProviderId::parse("bogus"), None);
        assert_eq!(ProviderId::parse("OpenAI"), Some(ProviderId::OpenAI));
        assert_eq!(ProviderId::parse("ANTHROPIC"), Some(ProviderId::Anthropic));
    }

    #[test]
    fn debug_output_never_contains_secret_value() {
        let store = MemorySecretStore::default();
        store
            .set(provider_scope(), SYNTHETIC_KEY)
            .expect("injected store accepts the write");

        let rendered = [
            format!("{:?}", store),
            format!("{:?}", UnconfiguredSecretStore),
            format!("{:?}", KeyringSecretStore::default()),
        ];
        for output in rendered {
            assert!(!output.contains("sk-"));
        }
    }

    #[test]
    fn unconfigured_store_rejects_writes_and_deletes() {
        let store = UnconfiguredSecretStore;
        let expected = Err(NO_OS_SECRET_STORE_ERROR.to_string());

        assert_eq!(store.set(provider_scope(), SYNTHETIC_KEY), expected.clone());
        assert_eq!(store.delete(provider_scope()), expected);
        assert_eq!(store.get(provider_scope()), None);
    }

    #[test]
    fn scope_keys_match_the_keyring_namespace() {
        assert_eq!(
            SecretScope::Provider(ProviderId::OpenAI).key(),
            "provider:openai"
        );
        assert_eq!(SecretScope::LanToken.key(), "lan-token");
    }
}
