//! Resolves image providers, reads credentials from the injected secret store,
//! and dispatches to the OpenAI-compatible `b64_json` backends. This registry
//! never constructs a local generation engine.

use crate::imagegen::openai_compat::B64JsonImageBackend;
use crate::imagegen::{
    GenerateImageRequest, ImageBackend, ImageError, ImageFuture, ImageGeneration, ImageProviderId,
    DEFAULT_GEMINI_IMAGES_URL, DEFAULT_GEMINI_IMAGE_MODEL, DEFAULT_OPENAI_IMAGES_URL,
    DEFAULT_OPENAI_IMAGE_MODEL,
};
use crate::secrets::{SecretScope, SecretStore};
use std::sync::Arc;

pub struct ImageBackendRegistry {
    /// `ImageProviderId` has no `Hash` impl, so a small ordered list serves the
    /// same lookup role as the planned map without changing the committed enum.
    backends: Vec<(ImageProviderId, Arc<dyn ImageBackend>)>,
    secrets: Arc<dyn SecretStore>,
    local_flag: bool,
}

impl ImageBackendRegistry {
    pub fn new(
        secrets: Arc<dyn SecretStore>,
        openai_endpoint: impl Into<String>,
        gemini_endpoint: impl Into<String>,
        local_flag: bool,
    ) -> Self {
        let backends: Vec<(ImageProviderId, Arc<dyn ImageBackend>)> = vec![
            (
                ImageProviderId::OpenAI,
                Arc::new(B64JsonImageBackend::new(
                    ImageProviderId::OpenAI,
                    DEFAULT_OPENAI_IMAGE_MODEL,
                    openai_endpoint,
                )),
            ),
            (
                ImageProviderId::Gemini,
                Arc::new(B64JsonImageBackend::new(
                    ImageProviderId::Gemini,
                    DEFAULT_GEMINI_IMAGE_MODEL,
                    gemini_endpoint,
                )),
            ),
        ];
        Self {
            backends,
            secrets,
            local_flag,
        }
    }

    pub fn from_env(secrets: Arc<dyn SecretStore>) -> Self {
        let openai_endpoint = std::env::var("AI_UPCYCLER_OPENAI_IMAGES_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_OPENAI_IMAGES_URL.to_string());
        let gemini_endpoint = std::env::var("AI_UPCYCLER_GEMINI_IMAGES_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_GEMINI_IMAGES_URL.to_string());
        let local_flag = std::env::var("AI_UPCYCLER_IMAGEGEN_LOCAL").as_deref() == Ok("1");
        Self::new(secrets, openai_endpoint, gemini_endpoint, local_flag)
    }

    pub fn generate<'a>(
        &'a self,
        request: &'a GenerateImageRequest,
    ) -> ImageFuture<'a, ImageGeneration> {
        Box::pin(async move {
            request.validate()?;
            match request.provider {
                ImageProviderId::Local if !self.local_flag => Err(ImageError::LocalDisabled),
                ImageProviderId::Local => Err(ImageError::LocalEngineMissing),
                provider => {
                    let provider_id = provider
                        .cloud_provider_id()
                        .ok_or(ImageError::UnsupportedProvider)?;
                    // Credential check happens before any backend call, so a
                    // missing key can never produce a network request.
                    let api_key = self
                        .secrets
                        .get(SecretScope::Provider(provider_id))
                        .filter(|value| !value.is_empty())
                        .ok_or(ImageError::ProviderNotConfigured(provider.as_str()))?;
                    let backend = self
                        .backends
                        .iter()
                        .find(|(id, _)| *id == provider)
                        .map(|(_, backend)| backend)
                        .ok_or(ImageError::UnsupportedProvider)?;
                    let generated = backend.generate(request, &api_key).await?;
                    Ok(ImageGeneration {
                        provider,
                        model: backend_name(provider).to_string(),
                        generated,
                    })
                }
            }
        })
    }
}

fn backend_name(provider: ImageProviderId) -> &'static str {
    match provider {
        ImageProviderId::OpenAI => DEFAULT_OPENAI_IMAGE_MODEL,
        ImageProviderId::Gemini => DEFAULT_GEMINI_IMAGE_MODEL,
        ImageProviderId::Local => "none",
    }
}

#[cfg(test)]
mod tests {
    use crate::imagegen::{
        GenerateImageRequest, ImageError, ImageProviderId, ImageSize, DEFAULT_GEMINI_IMAGES_URL,
        DEFAULT_OPENAI_IMAGES_URL,
    };
    use crate::providers::ProviderId;
    use crate::secrets::{SecretScope, SecretStore};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::ImageBackendRegistry;

    #[derive(Clone, Default)]
    struct TestSecrets {
        values: Arc<Mutex<HashMap<String, String>>>,
    }

    impl TestSecrets {
        fn key(scope: SecretScope) -> String {
            match scope {
                SecretScope::Provider(ProviderId::OpenAI) => "provider:openai".to_string(),
                SecretScope::Provider(ProviderId::Anthropic) => "provider:anthropic".to_string(),
                SecretScope::Provider(ProviderId::Gemini) => "provider:gemini".to_string(),
                SecretScope::Provider(ProviderId::Kimi) => "provider:kimi".to_string(),
                SecretScope::LanToken => "lan-token".to_string(),
            }
        }
    }

    impl SecretStore for TestSecrets {
        fn get(&self, scope: SecretScope) -> Option<String> {
            self.values.lock().unwrap().get(&Self::key(scope)).cloned()
        }

        fn set(&self, scope: SecretScope, value: &str) -> Result<(), String> {
            self.values
                .lock()
                .unwrap()
                .insert(Self::key(scope), value.to_string());
            Ok(())
        }

        fn delete(&self, scope: SecretScope) -> Result<(), String> {
            self.values.lock().unwrap().remove(&Self::key(scope));
            Ok(())
        }
    }

    fn request(provider: ImageProviderId) -> GenerateImageRequest {
        GenerateImageRequest {
            prompt: "a red bicycle".into(),
            provider,
            size: ImageSize::Square1024,
        }
    }

    #[tokio::test]
    async fn local_flag_does_not_create_an_engine() {
        let disabled = ImageBackendRegistry::new(
            Arc::new(TestSecrets::default()),
            DEFAULT_OPENAI_IMAGES_URL,
            DEFAULT_GEMINI_IMAGES_URL,
            false,
        );
        assert!(matches!(
            disabled.generate(&request(ImageProviderId::Local)).await,
            Err(ImageError::LocalDisabled)
        ));

        let enabled = ImageBackendRegistry::new(
            Arc::new(TestSecrets::default()),
            DEFAULT_OPENAI_IMAGES_URL,
            DEFAULT_GEMINI_IMAGES_URL,
            true,
        );
        assert!(matches!(
            enabled.generate(&request(ImageProviderId::Local)).await,
            Err(ImageError::LocalEngineMissing)
        ));
    }

    #[tokio::test]
    async fn missing_secret_is_reported_before_network_call() {
        let registry = ImageBackendRegistry::new(
            Arc::new(TestSecrets::default()),
            "http://127.0.0.1:1/images/generations",
            "http://127.0.0.1:1/images/generations",
            false,
        );
        assert!(matches!(
            registry.generate(&request(ImageProviderId::OpenAI)).await,
            Err(ImageError::ProviderNotConfigured("openai"))
        ));
    }
}
