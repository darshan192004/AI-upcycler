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
