use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSettings {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub context_tokens: u32,
    pub max_tokens: u32,
    pub show_source_badges: bool,
    pub compact_mode: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            provider: None,
            model: None,
            context_tokens: 4_096,
            max_tokens: 1_024,
            show_source_badges: true,
            compact_mode: false,
        }
    }
}

impl AppSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(1_024..=65_536).contains(&self.context_tokens) {
            return Err("context_tokens must be between 1024 and 65536");
        }
        if !(1..=32_768).contains(&self.max_tokens) {
            return Err("max_tokens must be between 1 and 32768");
        }
        if self.max_tokens > self.context_tokens {
            return Err("max_tokens must not exceed context_tokens");
        }
        if !valid_optional_string(&self.provider) {
            return Err("provider must be omitted or a non-empty string of at most 200 characters");
        }
        if !valid_optional_string(&self.model) {
            return Err("model must be omitted or a non-empty string of at most 200 characters");
        }
        Ok(())
    }
}

fn valid_optional_string(value: &Option<String>) -> bool {
    match value.as_deref() {
        None => true,
        Some(text) => !text.trim().is_empty() && text.chars().count() <= 200,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_deterministic() {
        assert_eq!(
            AppSettings::default(),
            AppSettings {
                provider: None,
                model: None,
                context_tokens: 4_096,
                max_tokens: 1_024,
                show_source_badges: true,
                compact_mode: false,
            }
        );
    }

    #[test]
    fn rejects_ranges_without_echoing_values() {
        let invalid = AppSettings {
            context_tokens: 512,
            ..AppSettings::default()
        };
        assert_eq!(
            invalid.validate(),
            Err("context_tokens must be between 1024 and 65536")
        );

        let invalid = AppSettings {
            context_tokens: 2_048,
            max_tokens: 4_096,
            ..AppSettings::default()
        };
        assert_eq!(
            invalid.validate(),
            Err("max_tokens must not exceed context_tokens")
        );
    }

    #[test]
    fn json_with_secret_fields_is_rejected() {
        let json = r#"{"provider":"openai","model":"gpt-4o","context_tokens":4096,"max_tokens":1024,"show_source_badges":true,"compact_mode":false,"api_key":"sk-shared-never"}"#;
        let result: Result<AppSettings, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "unknown secret-bearing fields must not deserialize"
        );
    }
}
