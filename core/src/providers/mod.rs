//! Cloud provider identifiers.
//!
//! Task 2 only needs the parsed identifier that names a keychain scope. Task 3
//! expands this module with descriptors, models, and the provider registry.

/// Known cloud providers that can hold a credential in the OS keychain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    OpenAI,
    Anthropic,
    Gemini,
    Kimi,
}

impl ProviderId {
    /// Stable lowercase spelling used in keychain keys and API payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Kimi => "kimi",
        }
    }

    /// Parses a provider id case-insensitively. Unknown or empty input is
    /// rejected so [`crate::secrets::SecretScope::Provider`] can only carry a
    /// known provider.
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "openai" => Some(Self::OpenAI),
            "anthropic" => Some(Self::Anthropic),
            "gemini" => Some(Self::Gemini),
            "kimi" => Some(Self::Kimi),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{SecretScope, SecretStore};
    use std::sync::Arc;

    const SYNTHETIC_KEY: &str = "sk-test-do-not-leak";

    struct EmptySecrets;

    impl SecretStore for EmptySecrets {
        fn get(&self, _scope: SecretScope) -> Option<String> {
            None
        }

        fn set(&self, _scope: SecretScope, _value: &str) -> Result<(), String> {
            Ok(())
        }

        fn delete(&self, _scope: SecretScope) -> Result<(), String> {
            Ok(())
        }
    }

    struct OpenAiSecrets;

    impl SecretStore for OpenAiSecrets {
        fn get(&self, scope: SecretScope) -> Option<String> {
            match scope {
                SecretScope::Provider(ProviderId::OpenAI) => Some(SYNTHETIC_KEY.to_string()),
                _ => None,
            }
        }

        fn set(&self, _scope: SecretScope, _value: &str) -> Result<(), String> {
            Ok(())
        }

        fn delete(&self, _scope: SecretScope) -> Result<(), String> {
            Ok(())
        }
    }

    fn registry() -> ProviderRegistry {
        ProviderRegistry::new(Arc::new(EmptySecrets))
    }

    #[derive(Debug)]
    enum Expected {
        Local,
        Cloud(ProviderId, &'static str),
        BadRequest(&'static str),
    }

    /// Every resolution rule from the task brief, plus the rule that only a
    /// known provider prefix is parsed (local GGUF ids may contain a colon).
    #[test]
    fn resolve_table_covers_every_routing_rule() {
        let registry = registry();
        let cases: &[(&str, Option<&str>, Expected)] = &[
            ("llama-8b", None, Expected::Local),
            (
                "openai:gpt-4o",
                None,
                Expected::Cloud(ProviderId::OpenAI, "gpt-4o"),
            ),
            (
                "gpt-4o",
                Some("openai"),
                Expected::Cloud(ProviderId::OpenAI, "gpt-4o"),
            ),
            (
                "openai:gpt-4o",
                Some("openai"),
                Expected::Cloud(ProviderId::OpenAI, "gpt-4o"),
            ),
            (
                "openai:gpt-4o",
                Some("anthropic"),
                Expected::BadRequest("provider"),
            ),
            ("openai:unknown", None, Expected::BadRequest("unknown")),
            ("some-local.gguf", None, Expected::Local),
            ("gpt-4o", None, Expected::Local),
            // Unknown prefixes are part of a local id, not a cloud route.
            ("custom:some-local.gguf", None, Expected::Local),
            // Prefix parsing follows the case-insensitive ProviderId parser.
            (
                "OpenAI:gpt-4o",
                None,
                Expected::Cloud(ProviderId::OpenAI, "gpt-4o"),
            ),
        ];

        for (model, provider, expected) in cases {
            let result = registry.resolve(model, *provider);
            match expected {
                Expected::Local => {
                    assert_eq!(result, Ok(None), "model={model}, provider={provider:?}");
                }
                Expected::Cloud(id, want_model) => assert_eq!(
                    result,
                    Ok(Some(CloudRoute {
                        provider: *id,
                        model: (*want_model).to_string(),
                    })),
                    "model={model}, provider={provider:?}"
                ),
                Expected::BadRequest(fragment) => {
                    let error = result.expect_err("expected a BadRequest error");
                    assert!(
                        error.contains(fragment),
                        "error '{error}' must mention '{fragment}' (model={model}, provider={provider:?})"
                    );
                }
            }
        }
    }

    #[test]
    fn catalogs_match_the_spec() {
        let expected: &[(ProviderId, &[&str])] = &[
            (
                ProviderId::OpenAI,
                &["gpt-4o", "gpt-4o-mini", "gpt-4.1", "gpt-4.1-mini"],
            ),
            (
                ProviderId::Anthropic,
                &[
                    "claude-3-5-sonnet-latest",
                    "claude-3-5-haiku-latest",
                    "claude-3-7-sonnet-latest",
                ],
            ),
            (
                ProviderId::Gemini,
                &["gemini-2.0-flash", "gemini-2.5-flash", "gemini-2.5-pro"],
            ),
            (
                ProviderId::Kimi,
                &["moonshot-v1-8k", "kimi-k2-turbo-preview"],
            ),
        ];
        for (id, ids) in expected {
            let descriptor = descriptor_for(*id);
            let actual: Vec<&str> = descriptor.models.iter().map(|m| m.id).collect();
            assert_eq!(actual, *ids, "catalog for {}", id.as_str());
        }
    }

    #[test]
    fn statuses_never_expose_the_secret_value() {
        let registry = ProviderRegistry::new(Arc::new(OpenAiSecrets));
        let statuses = registry.statuses();

        assert_eq!(statuses.len(), ProviderId::ALL.len());
        let openai = statuses
            .iter()
            .find(|status| status.id == ProviderId::OpenAI)
            .expect("OpenAI status is present");
        assert!(openai.configured);
        assert!(!openai.models.is_empty());
        assert!(statuses
            .iter()
            .all(|status| status.name == descriptor_for(status.id).name));

        let json = serde_json::to_string(&statuses).expect("statuses serialize");
        assert!(!json.contains(SYNTHETIC_KEY), "JSON must not leak the key");
        let debug = format!("{statuses:?}");
        assert!(!debug.contains(SYNTHETIC_KEY), "Debug must not leak the key");
    }
}
