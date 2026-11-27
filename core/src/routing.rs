//! Request routing policy for the OpenAI-compatible chat endpoint.
//!
//! `provider` on a request is either a route policy (`auto`, `local`) or a
//! cloud provider id. Model-id prefix inference (`provider:model`) is applied
//! only when the caller omits `provider`; an explicit policy never silently
//! falls back to a different execution source.

use crate::providers::ProviderId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutePolicy {
    /// Prefer a real local engine; use a configured cloud provider when the
    /// local backend is only the labelled offline stub.
    Auto,
    /// Always use the local backend, even when it is the offline stub.
    Local,
    /// Always use this cloud provider.
    Cloud(ProviderId),
}

impl RoutePolicy {
    /// Parses the optional `provider` field. Empty and absent values mean
    /// `auto`; unknown names are rejected with a value-free error.
    pub fn parse(provider: Option<&str>) -> Result<Self, String> {
        match provider.map(str::trim) {
            None | Some("") | Some("auto") => Ok(Self::Auto),
            Some("local") => Ok(Self::Local),
            Some(value) => ProviderId::parse(value)
                .map(Self::Cloud)
                .ok_or_else(|| format!("unknown provider '{value}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_case_sensitive_and_trims_whitespace() {
        assert_eq!(
            RoutePolicy::parse(Some("  local  ")).unwrap(),
            RoutePolicy::Local
        );
        assert!(RoutePolicy::parse(Some("LOCAL")).is_err());
    }
}
