//! Cloud provider descriptors, static model catalogs, and the registry that
//! resolves model ids and dispatches to a provider adapter.

use crate::api::backend::{ChatMessage, GenerationChunk, GenerationRequest};
use crate::media::MediaStore;
use crate::secrets::{SecretScope, SecretStore};
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

mod anthropic;
mod gemini;
mod kimi;
pub mod messages;
mod openai;
pub(crate) mod openai_compat;

/// Known cloud providers that can hold a credential in the OS keychain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    OpenAI,
    Anthropic,
    Gemini,
    Kimi,
}

impl ProviderId {
    pub const ALL: [Self; 4] = [Self::OpenAI, Self::Anthropic, Self::Gemini, Self::Kimi];

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

/// One cloud model in a provider's static catalog.
#[derive(Debug, Clone, Copy)]
pub struct ProviderModel {
    pub id: &'static str,
    pub label: &'static str,
    pub supports_vision: bool,
}

/// Static provider facts. `models` and its capability flags are the single
/// source of truth for the list/capability surface; there are no redundant
/// `list_models`/`supports_vision`/`supports_tools` adapter methods.
#[derive(Debug, Clone, Copy)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub name: &'static str,
    pub base_url_env: &'static str,
    pub default_base_url: &'static str,
    pub models: &'static [ProviderModel],
    pub supports_tools: bool,
}

static OPENAI_MODELS: &[ProviderModel] = &[
    ProviderModel {
        id: "gpt-4o",
        label: "GPT-4o",
        supports_vision: true,
    },
    ProviderModel {
        id: "gpt-4o-mini",
        label: "GPT-4o mini",
        supports_vision: true,
    },
    ProviderModel {
        id: "gpt-4.1",
        label: "GPT-4.1",
        supports_vision: true,
    },
    ProviderModel {
        id: "gpt-4.1-mini",
        label: "GPT-4.1 mini",
        supports_vision: true,
    },
];

static ANTHROPIC_MODELS: &[ProviderModel] = &[
    ProviderModel {
        id: "claude-3-5-sonnet-latest",
        label: "Claude 3.5 Sonnet",
        supports_vision: true,
    },
    ProviderModel {
        id: "claude-3-5-haiku-latest",
        label: "Claude 3.5 Haiku",
        supports_vision: true,
    },
    ProviderModel {
        id: "claude-3-7-sonnet-latest",
        label: "Claude 3.7 Sonnet",
        supports_vision: true,
    },
];

static GEMINI_MODELS: &[ProviderModel] = &[
    ProviderModel {
        id: "gemini-2.0-flash",
        label: "Gemini 2.0 Flash",
        supports_vision: true,
    },
    ProviderModel {
        id: "gemini-2.5-flash",
        label: "Gemini 2.5 Flash",
        supports_vision: true,
    },
    ProviderModel {
        id: "gemini-2.5-pro",
        label: "Gemini 2.5 Pro",
        supports_vision: true,
    },
];

static KIMI_MODELS: &[ProviderModel] = &[
    ProviderModel {
        id: "moonshot-v1-8k",
        label: "Moonshot v1 8K",
        supports_vision: false,
    },
    ProviderModel {
        id: "kimi-k2-turbo-preview",
        label: "Kimi K2 Turbo Preview",
        supports_vision: false,
    },
];

static OPENAI_DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ProviderId::OpenAI,
    name: "OpenAI",
    base_url_env: "OPENAI_BASE_URL",
    default_base_url: "https://api.openai.com/v1",
    models: OPENAI_MODELS,
    supports_tools: false,
};

static ANTHROPIC_DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ProviderId::Anthropic,
    name: "Anthropic",
    base_url_env: "ANTHROPIC_BASE_URL",
    default_base_url: "https://api.anthropic.com",
    models: ANTHROPIC_MODELS,
    supports_tools: false,
};

static GEMINI_DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ProviderId::Gemini,
    name: "Google Gemini",
    base_url_env: "GEMINI_BASE_URL",
    default_base_url: "https://generativelanguage.googleapis.com",
    models: GEMINI_MODELS,
    supports_tools: false,
};

static KIMI_DESCRIPTOR: ProviderDescriptor = ProviderDescriptor {
    id: ProviderId::Kimi,
    name: "Kimi (Moonshot)",
    base_url_env: "KIMI_BASE_URL",
    default_base_url: "https://api.moonshot.ai/v1",
    models: KIMI_MODELS,
    supports_tools: false,
};

/// The static descriptor for a provider id.
pub fn descriptor_for(id: ProviderId) -> &'static ProviderDescriptor {
    match id {
        ProviderId::OpenAI => &OPENAI_DESCRIPTOR,
        ProviderId::Anthropic => &ANTHROPIC_DESCRIPTOR,
        ProviderId::Gemini => &GEMINI_DESCRIPTOR,
        ProviderId::Kimi => &KIMI_DESCRIPTOR,
    }
}

/// Serializable model entry for a provider status response. Never carries a
/// credential, a request, or any response text.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderModelWire {
    pub id: String,
    pub label: String,
    pub supports_vision: bool,
}

/// Serializable, credential-free provider status.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderStatus {
    pub id: ProviderId,
    pub name: &'static str,
    pub configured: bool,
    pub models: Vec<ProviderModelWire>,
    pub supports_vision: bool,
    pub supports_tools: bool,
}

/// A resolved cloud destination. `model` is the bare provider model id; the
/// request's original id stays on the API envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudRoute {
    pub provider: ProviderId,
    pub model: String,
}

/// Transport for one provider. Capabilities live in the descriptor, not here.
pub trait CloudProvider: Send + Sync + 'static {
    fn id(&self) -> ProviderId;
    fn descriptor(&self) -> &'static ProviderDescriptor;
    fn chat_stream(
        &self,
        request: GenerationRequest,
        api_key: &str,
    ) -> BoxStream<'static, Result<GenerationChunk, String>>;
}

/// Resolves routes, reads credentials from the injected secret store, and
/// dispatches to the matching adapter.
pub struct ProviderRegistry {
    secrets: Arc<dyn SecretStore>,
    base_urls: HashMap<ProviderId, String>,
    /// Shared HTTP transport. Production uses reqwest's normal proxy
    /// behaviour; tests inject a `.no_proxy()` client for loopback mocks.
    client: reqwest::Client,
    adapters: HashMap<ProviderId, Box<dyn CloudProvider>>,
    /// Shared store used by adapters to resolve image parts into bytes. The
    /// `Arc` keeps builder-installed and state-installed stores identical.
    media: Option<Arc<MediaStore>>,
}

impl fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderRegistry")
            .field("providers", &ProviderId::ALL)
            .finish_non_exhaustive()
    }
}

impl ProviderRegistry {
    /// Builds all four providers, reading `<ID>_BASE_URL` with a static
    /// fallback. No network call is made here.
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        let mut base_urls = HashMap::new();
        for id in ProviderId::ALL {
            let descriptor = descriptor_for(id);
            let from_env = std::env::var(descriptor.base_url_env)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            base_urls.insert(
                id,
                from_env.unwrap_or_else(|| descriptor.default_base_url.to_string()),
            );
        }
        let mut registry = Self {
            secrets,
            base_urls,
            client: reqwest::Client::new(),
            adapters: HashMap::new(),
            media: None,
        };
        registry.rebuild_adapters();
        registry
    }

    /// Overrides one provider's base URL (used by adapter tests with a mock
    /// server). No secret value is accepted or stored here.
    pub fn with_base_url(mut self, id: ProviderId, url: impl Into<String>) -> Self {
        self.base_urls.insert(id, url.into());
        self.rebuild_adapters();
        self
    }

    /// Replaces the HTTP client used by every adapter. Tests inject a client
    /// built with `.no_proxy()` so loopback mocks are never routed through a
    /// system proxy; production keeps the default client from [`Self::new`].
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self.rebuild_adapters();
        self
    }

    /// Installs the content-addressed image store shared with the API layer.
    /// Adapters are rebuilt so every provider sees the same store instance.
    pub fn with_media_store(mut self, media: MediaStore) -> Self {
        self.media = Some(Arc::new(media));
        self.rebuild_adapters();
        self
    }

    /// Rebuilds a registry around `Arc<Self>` without cloning adapter objects.
    /// `AppState` builders use this so a media-root override reaches adapters
    /// even when a custom registry was injected first.
    pub(crate) fn with_media_store_shared(&self, media: Arc<MediaStore>) -> Arc<Self> {
        let mut registry = Self {
            secrets: self.secrets.clone(),
            base_urls: self.base_urls.clone(),
            client: self.client.clone(),
            adapters: HashMap::new(),
            media: Some(media),
        };
        registry.rebuild_adapters();
        Arc::new(registry)
    }

    /// The effective base URL for a provider, after env/default resolution.
    pub fn base_url(&self, id: ProviderId) -> &str {
        self.base_urls
            .get(&id)
            .map(String::as_str)
            .unwrap_or_else(|| descriptor_for(id).default_base_url)
    }

    fn rebuild_adapters(&mut self) {
        self.adapters.clear();
        for id in ProviderId::ALL {
            let base_url = self.base_urls[&id].clone();
            let media = self.media.clone();
            let adapter: Box<dyn CloudProvider> = match (id, media) {
                (ProviderId::OpenAI, Some(media)) => {
                    Box::new(openai::provider(self.client.clone(), base_url, Some(media)))
                }
                (ProviderId::OpenAI, None) => {
                    Box::new(openai::compat_provider(id, self.client.clone(), base_url))
                }
                (ProviderId::Anthropic, media) => {
                    Box::new(anthropic::provider(self.client.clone(), base_url, media))
                }
                (ProviderId::Gemini, media) => {
                    Box::new(gemini::provider(self.client.clone(), base_url, media))
                }
                (ProviderId::Kimi, Some(media)) => {
                    Box::new(kimi::provider(self.client.clone(), base_url, Some(media)))
                }
                (ProviderId::Kimi, None) => {
                    Box::new(openai::compat_provider(id, self.client.clone(), base_url))
                }
            };
            self.adapters.insert(id, adapter);
        }
    }

    /// Resolves a request id to a cloud route, or `None` when it should be
    /// looked up locally. A `provider:model` prefix is parsed only when the
    /// prefix is a known provider, so local GGUF ids containing a colon stay
    /// local.
    pub fn resolve(
        &self,
        model: &str,
        provider: Option<&str>,
    ) -> Result<Option<CloudRoute>, String> {
        let explicit = provider.map(str::trim).filter(|value| !value.is_empty());
        match explicit {
            // `auto` and `local` are route policies, not cloud provider ids.
            // `auto` leaves the decision to the caller (Task 7's RoutePolicy);
            // model-prefix inference is only applied when provider is absent.
            Some("auto") | Some("local") => Ok(None),
            Some(value) => {
                let id = ProviderId::parse(value)
                    .ok_or_else(|| format!("unknown provider '{value}'"))?;
                if let Some((prefix, _)) = split_model(model) {
                    if prefix != id {
                        return Err(format!(
                            "model '{model}' belongs to provider '{}', not '{value}'",
                            prefix.as_str()
                        ));
                    }
                }
                let bare = bare_model(model).to_string();
                if !descriptor_for(id)
                    .models
                    .iter()
                    .any(|entry| entry.id == bare)
                {
                    return Err(format!(
                        "unknown model '{bare}' for provider '{}'",
                        id.as_str()
                    ));
                }
                Ok(Some(CloudRoute {
                    provider: id,
                    model: bare,
                }))
            }
            None => {
                if let Some((id, bare)) = split_model(model) {
                    if !descriptor_for(id)
                        .models
                        .iter()
                        .any(|entry| entry.id == bare)
                    {
                        return Err(format!(
                            "unknown model '{bare}' for provider '{}'",
                            id.as_str()
                        ));
                    }
                    return Ok(Some(CloudRoute {
                        provider: id,
                        model: bare.to_string(),
                    }));
                }
                Ok(None)
            }
        }
    }

    /// Credential-free provider statuses. `configured` is derived only from
    /// whether the secret store has a value; the value is never copied here.
    pub fn statuses(&self) -> Vec<ProviderStatus> {
        ProviderId::ALL
            .iter()
            .map(|&id| {
                let descriptor = descriptor_for(id);
                ProviderStatus {
                    id,
                    name: descriptor.name,
                    configured: self.secrets.get(SecretScope::Provider(id)).is_some(),
                    models: descriptor
                        .models
                        .iter()
                        .map(|model| ProviderModelWire {
                            id: model.id.to_string(),
                            label: model.label.to_string(),
                            supports_vision: model.supports_vision,
                        })
                        .collect(),
                    supports_vision: descriptor.models.iter().any(|model| model.supports_vision),
                    supports_tools: descriptor.supports_tools,
                }
            })
            .collect()
    }

    /// Streams one turn through the resolved provider's adapter. Absent or
    /// empty credentials produce a value-free error stream.
    pub fn stream(
        &self,
        route: &CloudRoute,
        request: GenerationRequest,
    ) -> BoxStream<'static, Result<GenerationChunk, String>> {
        let api_key = match self
            .secrets
            .get(SecretScope::Provider(route.provider))
            .filter(|value| !value.is_empty())
        {
            Some(key) => key,
            None => {
                let message = format!("{} is not configured", route.provider.as_str());
                return Box::pin(async_stream::stream! {
                    yield Err(message);
                });
            }
        };
        match self.adapters.get(&route.provider) {
            Some(adapter) => adapter.chat_stream(request, &api_key),
            None => {
                let message = format!("{} provider is unavailable", route.provider.as_str());
                Box::pin(async_stream::stream! {
                    yield Err(message);
                })
            }
        }
    }

    /// Runs a fixed one-token prompt and returns only a truncated preview.
    pub async fn test(&self, id: ProviderId, model: &str) -> Result<String, String> {
        let api_key = self
            .secrets
            .get(SecretScope::Provider(id))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{} is not configured", id.as_str()))?;
        let adapter = self
            .adapters
            .get(&id)
            .ok_or_else(|| format!("{} provider is unavailable", id.as_str()))?;
        let request = GenerationRequest {
            model: model.to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "Reply with one token.".into(),
                ..Default::default()
            }],
            max_tokens: 1,
            ..Default::default()
        };
        let mut stream = adapter.chat_stream(request, &api_key);
        let mut preview = String::new();
        while let Some(chunk) = stream.next().await {
            preview.push_str(&chunk?.content);
            if !preview.is_empty() {
                break;
            }
        }
        Ok(truncate_preview(&preview))
    }
}

/// Splits a `provider:model` id only when the prefix is a known provider.
fn split_model(model: &str) -> Option<(ProviderId, &str)> {
    let (prefix, rest) = model.split_once(':')?;
    Some((ProviderId::parse(prefix)?, rest))
}

/// The bare model id, with a known prefix stripped when present.
pub(crate) fn bare_model(model: &str) -> &str {
    split_model(model).map(|(_, bare)| bare).unwrap_or(model)
}

fn truncate_preview(value: &str) -> String {
    let mut chars = value.chars();
    let preview: String = chars.by_ref().take(80).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
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
            // Explicit policy values are not rejected as provider ids.
            ("llama-8b", Some("auto"), Expected::Local),
            ("llama-8b", Some("local"), Expected::Local),
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
        assert!(
            !debug.contains(SYNTHETIC_KEY),
            "Debug must not leak the key"
        );
    }

    #[test]
    fn base_url_override_is_observable() {
        let registry = registry().with_base_url(ProviderId::OpenAI, "http://127.0.0.1:1234/v1");
        assert_eq!(
            registry.base_url(ProviderId::OpenAI),
            "http://127.0.0.1:1234/v1"
        );
        assert!(!registry.base_url(ProviderId::Kimi).is_empty());
    }

    #[tokio::test]
    async fn stream_without_key_returns_a_value_free_error() {
        let registry = registry();
        let route = CloudRoute {
            provider: ProviderId::OpenAI,
            model: "gpt-4o".to_string(),
        };
        let mut stream = registry.stream(&route, GenerationRequest::default());
        let error = stream
            .next()
            .await
            .expect("one chunk")
            .expect_err("missing key is an error chunk");
        assert!(error.contains("openai is not configured"), "{error}");
        assert!(
            !error.contains(SYNTHETIC_KEY),
            "error must not leak a value"
        );
    }
}
