use crate::providers::ProviderId;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

pub mod format;
pub mod media;

pub const DEFAULT_OPENAI_IMAGE_MODEL: &str = "gpt-image-2.6-flare";
pub const DEFAULT_GEMINI_IMAGE_MODEL: &str = "gemini-2.5-flash-image";
pub const DEFAULT_OPENAI_IMAGES_URL: &str = "https://api.openai.com/v1/images/generations";
pub const DEFAULT_GEMINI_IMAGES_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta/openai/images/generations";
pub const INVALID_PROMPT_MESSAGE: &str = "prompt must be 1..=4000 non-whitespace characters";
pub const LOCAL_DISABLED_MESSAGE: &str =
    "local image generation is disabled; set AI_UPCYCLER_IMAGEGEN_LOCAL=1 after adding a local image engine";
pub const LOCAL_ENGINE_MESSAGE: &str =
    "local image generation requires a new engine (for example stable-diffusion.cpp); sub-project F does not provide one";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageProviderId {
    OpenAI,
    Gemini,
    Local,
}

impl ImageProviderId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Gemini => "gemini",
            Self::Local => "local",
        }
    }

    pub fn cloud_provider_id(self) -> Option<ProviderId> {
        match self {
            Self::OpenAI => Some(ProviderId::OpenAI),
            Self::Gemini => Some(ProviderId::Gemini),
            Self::Local => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSize {
    Square1024,
    Portrait1024x1536,
    Landscape1536x1024,
}

impl ImageSize {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Square1024 => "1024x1024",
            Self::Portrait1024x1536 => "1024x1536",
            Self::Landscape1536x1024 => "1536x1024",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateImageRequest {
    pub prompt: String,
    pub provider: ImageProviderId,
    pub size: ImageSize,
}

impl GenerateImageRequest {
    pub fn validate(&self) -> Result<(), ImageError> {
        let trimmed = self.prompt.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 4000 {
            Err(ImageError::InvalidPrompt)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedImageBytes {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub extension: String,
}

pub struct ImageGeneration {
    pub provider: ImageProviderId,
    pub model: String,
    pub generated: GeneratedImageBytes,
}

pub type ImageFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ImageError>> + Send + 'a>>;

pub trait ImageBackend: Send + Sync {
    fn provider(&self) -> ImageProviderId;
    fn generate<'a>(
        &'a self,
        request: &'a GenerateImageRequest,
        api_key: &'a str,
    ) -> ImageFuture<'a, GeneratedImageBytes>;
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("prompt must be 1..=4000 non-whitespace characters")]
    InvalidPrompt,
    #[error("unsupported image provider")]
    UnsupportedProvider,
    #[error("local image generation is disabled; set AI_UPCYCLER_IMAGEGEN_LOCAL=1 after adding a local image engine")]
    LocalDisabled,
    #[error("local image generation requires a new engine (for example stable-diffusion.cpp); sub-project F does not provide one")]
    LocalEngineMissing,
    #[error("{0} image provider is not configured")]
    ProviderNotConfigured(&'static str),
    #[error("upstream image provider returned HTTP {0}")]
    UpstreamStatus(u16),
    #[error("upstream image response was invalid: {0}")]
    UpstreamBody(String),
    #[error("image generation returned no images")]
    EmptyResponse,
    #[error("image bytes are not PNG, JPEG, or WebP")]
    UnsupportedFormat,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_prompt_bounds() {
        let request = GenerateImageRequest {
            prompt: "   ".into(),
            provider: ImageProviderId::OpenAI,
            size: ImageSize::Square1024,
        };
        assert!(matches!(request.validate(), Err(ImageError::InvalidPrompt)));

        let request = GenerateImageRequest {
            prompt: "a".repeat(4000),
            provider: ImageProviderId::OpenAI,
            size: ImageSize::Square1024,
        };
        assert!(request.validate().is_ok());

        let request = GenerateImageRequest {
            prompt: "a".repeat(4001),
            provider: ImageProviderId::OpenAI,
            size: ImageSize::Square1024,
        };
        assert!(matches!(request.validate(), Err(ImageError::InvalidPrompt)));
    }

    #[test]
    fn maps_size_and_provider_strings() {
        assert_eq!(ImageSize::Portrait1024x1536.as_str(), "1024x1536");
        assert_eq!(ImageProviderId::Gemini.as_str(), "gemini");
        assert_eq!(ImageProviderId::Local.cloud_provider_id(), None);
    }
}
