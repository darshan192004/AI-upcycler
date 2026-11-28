//! OpenAI-compatible `b64_json` image generation backend.
//!
//! The same wire shape serves OpenAI and Gemini's OpenAI-compatible images
//! endpoint. Error strings for transport, decode, and base64 failures are
//! fixed literals so upstream bodies, URLs, and credentials can never be
//! echoed back to the caller.

use crate::imagegen::format::sniff_image;
use crate::imagegen::{
    GenerateImageRequest, GeneratedImageBytes, ImageBackend, ImageError, ImageFuture,
    ImageProviderId,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub struct B64JsonImageBackend {
    provider: ImageProviderId,
    model: &'static str,
    endpoint: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    n: u8,
    size: &'a str,
    /// Gemini's OpenAI-compatible images endpoint needs this hint; OpenAI's
    /// native endpoint rejects it, so it is omitted for OpenAI.
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'static str>,
}

#[derive(Deserialize)]
struct WireResponse {
    data: Vec<WireImage>,
}

#[derive(Deserialize)]
struct WireImage {
    b64_json: String,
}

impl B64JsonImageBackend {
    pub fn new(
        provider: ImageProviderId,
        model: &'static str,
        endpoint: impl Into<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120))
            .build()
            .expect("valid imagegen HTTP client configuration");
        Self::with_client(provider, model, endpoint, client)
    }

    pub fn with_client(
        provider: ImageProviderId,
        model: &'static str,
        endpoint: impl Into<String>,
        client: reqwest::Client,
    ) -> Self {
        Self {
            provider,
            model,
            endpoint: endpoint.into(),
            client,
        }
    }
}

impl ImageBackend for B64JsonImageBackend {
    fn provider(&self) -> ImageProviderId {
        self.provider
    }

    fn generate<'a>(
        &'a self,
        request: &'a GenerateImageRequest,
        api_key: &'a str,
    ) -> ImageFuture<'a, GeneratedImageBytes> {
        Box::pin(async move {
            request.validate()?;
            let response = self
                .client
                .post(&self.endpoint)
                .header("Authorization", format!("Bearer {api_key}"))
                .json(&WireRequest {
                    model: self.model,
                    prompt: request.prompt.trim(),
                    n: 1,
                    size: request.size.as_str(),
                    response_format: match self.provider {
                        ImageProviderId::Gemini => Some("b64_json"),
                        ImageProviderId::OpenAI | ImageProviderId::Local => None,
                    },
                })
                .send()
                .await
                .map_err(|_| ImageError::UpstreamBody("image provider request failed".into()))?;

            let status = response.status();
            if !status.is_success() {
                return Err(ImageError::UpstreamStatus(status.as_u16()));
            }

            let wire: WireResponse = response
                .json()
                .await
                .map_err(|_| ImageError::UpstreamBody("invalid image provider response".into()))?;
            let first = wire
                .data
                .into_iter()
                .next()
                .ok_or(ImageError::EmptyResponse)?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(first.b64_json)
                .map_err(|_| ImageError::UpstreamBody("invalid base64 image payload".into()))?;
            sniff_image(&bytes)
        })
    }
}
