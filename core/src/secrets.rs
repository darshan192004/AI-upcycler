//! OS-keychain-backed secret storage.
//!
//! Provider credentials live only in the native credential store. This module
//! never writes a secret value to SQLite, logs, telemetry, or API responses.

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
        assert_eq!(
            ProviderId::parse("ANTHROPIC"),
            Some(ProviderId::Anthropic)
        );
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

        assert_eq!(
            store.set(provider_scope(), SYNTHETIC_KEY),
            expected.clone()
        );
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
