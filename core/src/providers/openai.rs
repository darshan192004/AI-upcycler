//! OpenAI adapter. Its wire protocol is the shared OpenAI-compatible helper,
//! with image parts resolved through the API's content-addressed media store.

use super::messages::to_openai_message;
use super::openai_compat::take_complete_lines;
use super::{bare_model, descriptor_for, CloudProvider, ProviderDescriptor, ProviderId};
use crate::api::backend::{
    ChatMessage, ContentPart, GenerationChunk, GenerationRequest, MessageContent,
};
use crate::media::{MediaError, MediaStore};
use futures::stream::{BoxStream, StreamExt};
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// OpenAI-compatible adapter that can resolve image parts to data URLs.
#[derive(Clone)]
pub(crate) struct MediaOpenAiCompatProvider {
    id: ProviderId,
    client: reqwest::Client,
    base_url: String,
    media: Option<Arc<MediaStore>>,
}

impl MediaOpenAiCompatProvider {
    pub(crate) fn new(
        id: ProviderId,
        client: reqwest::Client,
        base_url: String,
        media: Option<Arc<MediaStore>>,
    ) -> Self {
        Self {
            id,
            client,
            base_url,
            media,
        }
    }
}

impl CloudProvider for MediaOpenAiCompatProvider {
    fn id(&self) -> ProviderId {
        self.id
    }

    fn descriptor(&self) -> &'static ProviderDescriptor {
        descriptor_for(self.id)
    }

    fn chat_stream(
        &self,
        request: GenerationRequest,
        api_key: &str,
    ) -> BoxStream<'static, Result<GenerationChunk, String>> {
        openai_compat_stream_with_media(
            self.client.clone(),
            &self.base_url,
            self.id,
            api_key,
            request,
            self.media.clone(),
        )
    }
}

pub(crate) fn provider(
    client: reqwest::Client,
    base_url: String,
    media: Option<Arc<MediaStore>>,
) -> MediaOpenAiCompatProvider {
    MediaOpenAiCompatProvider::new(ProviderId::OpenAI, client, base_url, media)
}

/// The pre-media transport remains the fallback for registries that were not
/// installed into `AppState`. It keeps A's text-only behaviour byte-for-byte
/// while media-backed states use [`provider`] above.
pub(crate) fn compat_provider(
    id: ProviderId,
    client: reqwest::Client,
    base_url: String,
) -> super::openai_compat::OpenAiCompatProvider {
    super::openai_compat::OpenAiCompatProvider::new(id, client, base_url)
}

/// Maps each message through the committed pure mapper. A message with several
/// images is mapped one part at a time so every part gets its own data URL;
/// no image bytes or encoded URLs are logged.
async fn messages_payload(
    media: Option<&Arc<MediaStore>>,
    messages: &[ChatMessage],
) -> Result<Vec<Value>, MediaError> {
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        if !message.content.has_images() {
            out.push(to_openai_message(message, ""));
            continue;
        }

        let MessageContent::Parts(parts) = &message.content else {
            continue;
        };
        let mut content = Vec::with_capacity(parts.len());
        for part in parts {
            let data_url = match part {
                ContentPart::Text { .. } => String::new(),
                ContentPart::Image {
                    media_hash,
                    mime_type,
                    ..
                } => {
                    let store = media.ok_or(MediaError::Io)?;
                    // Resolve the bytes first so a missing or drifting file
                    // fails closed before any provider request is built.
                    store.get(media_hash).await?;
                    store.data_url(media_hash, mime_type).await?
                }
            };
            let single = ChatMessage {
                role: message.role.clone(),
                content: MessageContent::Parts(vec![part.clone()]),
                ..Default::default()
            };
            let mapped = to_openai_message(&single, &data_url);
            if let Some(block) = mapped
                .get("content")
                .and_then(Value::as_array)
                .and_then(|blocks| blocks.first())
            {
                content.push(block.clone());
            }
        }
        out.push(json!({ "role": &message.role, "content": content }));
    }
    Ok(out)
}

async fn payload(
    media: Option<&Arc<MediaStore>>,
    request: &GenerationRequest,
) -> Result<Value, MediaError> {
    let mut body = Map::new();
    body.insert("model".to_string(), json!(bare_model(&request.model)));
    body.insert(
        "messages".to_string(),
        json!(messages_payload(media, &request.messages).await?),
    );
    body.insert("temperature".to_string(), json!(request.temperature));
    body.insert("max_tokens".to_string(), json!(request.max_tokens));
    body.insert("stream".to_string(), json!(true));
    if let Some(stop) = &request.stop {
        body.insert("stop".to_string(), json!(stop));
    }
    if let Some(top_p) = request.top_p {
        body.insert("top_p".to_string(), json!(top_p));
    }
    if let Some(seed) = request.seed {
        body.insert("seed".to_string(), json!(seed));
    }
    Ok(Value::Object(body))
}

/// Streams one OpenAI-compatible chat turn through a media-aware adapter.
///
/// Errors name only the provider and (for HTTP failures) the status code. The
/// key, request body, response body, image bytes and data URLs are never put
/// into an error, so a provider failure cannot leak a credential or an image.
pub(crate) fn openai_compat_stream_with_media(
    client: reqwest::Client,
    base_url: &str,
    provider: ProviderId,
    api_key: &str,
    request: GenerationRequest,
    media: Option<Arc<MediaStore>>,
) -> BoxStream<'static, Result<GenerationChunk, String>> {
    let provider_name = provider.as_str().to_string();
    let api_key = api_key.to_string();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    Box::pin(async_stream::stream! {
        let body = match payload(media.as_ref(), &request).await {
            Ok(body) => body,
            Err(_) => {
                yield Err("invalid_image_attachment".to_string());
                return;
            }
        };

        let response = match client
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                yield Err(format!("{provider_name} request failed"));
                return;
            }
        };

        let status = response.status();
        if !status.is_success() {
            yield Err(format!("{provider_name} returned HTTP {}", status.as_u16()));
            return;
        }

        let mut bytes = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        while let Some(item) = bytes.next().await {
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(_) => {
                    yield Err(format!("{provider_name} stream failed"));
                    return;
                }
            };
            for line in take_complete_lines(&mut buffer, &chunk) {
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" {
                    continue;
                }
                let parsed: Value = match serde_json::from_str(data) {
                    Ok(parsed) => parsed,
                    Err(_) => {
                        yield Err(format!("{provider_name} returned an unreadable response"));
                        return;
                    }
                };
                let choice = parsed.get("choices").and_then(Value::as_array).and_then(|items| items.first());
                let content = choice
                    .and_then(|item| item.get("delta"))
                    .and_then(|delta| delta.get("content"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let finish_reason = choice
                    .and_then(|item| item.get("finish_reason"))
                    .and_then(Value::as_str)
                    .map(str::to_string);

                if !content.is_empty() {
                    yield Ok(GenerationChunk {
                        content,
                        tokens_emitted: 1,
                        source: Some(provider_name.clone()),
                        ..Default::default()
                    });
                }
                if let Some(finish_reason) = finish_reason {
                    yield Ok(GenerationChunk {
                        finish_reason: Some(finish_reason),
                        source: Some(provider_name.clone()),
                        ..Default::default()
                    });
                }
            }
        }
    })
}
