//! Anthropic Messages adapter.
//!
//! Anthropic's streaming protocol differs from OpenAI's: the credential goes
//! in `x-api-key`, system turns live in a top-level `system` string, and SSE
//! events carry an `event:` line before each `data:` line. The shared SSE line
//! buffer from `openai_compat` is reused; only the event dispatch is new. Image
//! bytes are resolved only through the shared media store and are passed to the
//! committed message mapper as raw base64.

use super::messages::to_anthropic_content;
use super::openai_compat::take_complete_lines;
use super::{bare_model, descriptor_for, CloudProvider, ProviderDescriptor, ProviderId};
use crate::api::backend::{
    ChatMessage, ContentPart, GenerationChunk, GenerationRequest, MessageContent, Stop,
};
use crate::media::{MediaError, MediaStore};
use futures::stream::{BoxStream, StreamExt};
use serde_json::{json, Map, Value};
use std::sync::Arc;

const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Adapter backed by Anthropic's `/v1/messages` streaming endpoint.
#[derive(Clone)]
pub(crate) struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    media: Option<Arc<MediaStore>>,
}

impl AnthropicProvider {
    pub(crate) fn new(
        client: reqwest::Client,
        base_url: String,
        media: Option<Arc<MediaStore>>,
    ) -> Self {
        Self {
            client,
            base_url,
            media,
        }
    }
}

pub(crate) fn provider(
    client: reqwest::Client,
    base_url: String,
    media: Option<Arc<MediaStore>>,
) -> AnthropicProvider {
    AnthropicProvider::new(client, base_url, media)
}

impl CloudProvider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Anthropic
    }

    fn descriptor(&self) -> &'static ProviderDescriptor {
        descriptor_for(ProviderId::Anthropic)
    }

    fn chat_stream(
        &self,
        request: GenerationRequest,
        api_key: &str,
    ) -> BoxStream<'static, Result<GenerationChunk, String>> {
        anthropic_stream(
            self.client.clone(),
            &self.base_url,
            api_key,
            request,
            self.media.clone(),
        )
    }
}

/// Maps one message to Anthropic content blocks. Each image gets its own
/// source block so a multi-image turn cannot reuse the first image's payload.
async fn mapped_content(
    media: Option<&Arc<MediaStore>>,
    message: &ChatMessage,
) -> Result<Vec<Value>, MediaError> {
    if !message.content.has_images() {
        return Ok(to_anthropic_content(message, ""));
    }
    let MessageContent::Parts(parts) = &message.content else {
        return Ok(to_anthropic_content(message, ""));
    };

    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        let base64_data = match part {
            ContentPart::Text { .. } => String::new(),
            ContentPart::Image {
                media_hash,
                mime_type,
                ..
            } => {
                let store = media.ok_or(MediaError::Io)?;
                // Read first so a missing or drifting file fails closed before
                // any provider request is built.
                store.get(media_hash).await?;
                let data_url = store.data_url(media_hash, mime_type).await?;
                data_url
                    .split_once(',')
                    .map(|(_, data)| data.to_string())
                    .unwrap_or_default()
            }
        };
        let single = ChatMessage {
            role: message.role.clone(),
            content: MessageContent::Parts(vec![part.clone()]),
            ..Default::default()
        };
        out.extend(to_anthropic_content(&single, &base64_data));
    }
    Ok(out)
}

/// Anthropic accepts only `user` and `assistant` turns. System messages are
/// joined into the dedicated top-level field, and any other role is treated as
/// a user turn so a cloud route can never emit an invalid role.
async fn anthropic_payload(
    media: Option<&Arc<MediaStore>>,
    request: &GenerationRequest,
) -> Result<Value, MediaError> {
    let system = request
        .messages
        .iter()
        .filter(|message| message.role == "system")
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let mut messages: Vec<Value> = Vec::new();
    for message in request
        .messages
        .iter()
        .filter(|message| message.role != "system")
    {
        messages.push(json!({
            "role": if message.role == "assistant" { "assistant" } else { "user" },
            "content": mapped_content(media, message).await?,
        }));
    }

    let mut body = Map::new();
    body.insert("model".to_string(), json!(bare_model(&request.model)));
    body.insert("system".to_string(), json!(system));
    body.insert("messages".to_string(), json!(messages));
    body.insert("max_tokens".to_string(), json!(request.max_tokens));
    body.insert("temperature".to_string(), json!(request.temperature));
    body.insert("stream".to_string(), json!(true));
    if let Some(stop) = &request.stop {
        let sequences = match stop {
            Stop::One(value) => vec![value.clone()],
            Stop::Many(values) => values.clone(),
        };
        body.insert("stop_sequences".to_string(), json!(sequences));
    }
    Ok(Value::Object(body))
}

/// Builds the messages URL. A base url that already ends in `/v1` is
/// accepted so an env override does not produce a duplicated path segment.
fn messages_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        format!("{trimmed}/messages")
    } else {
        format!("{trimmed}/v1/messages")
    }
}

/// Streams one Anthropic chat turn. Errors name only the provider and, for
/// HTTP failures, the numeric status; no key, request body, or response body
/// is ever formatted into an error.
fn anthropic_stream(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: GenerationRequest,
    media: Option<Arc<MediaStore>>,
) -> BoxStream<'static, Result<GenerationChunk, String>> {
    let provider_name = ProviderId::Anthropic.as_str().to_string();
    let api_key = api_key.to_string();
    let url = messages_url(base_url);

    Box::pin(async_stream::stream! {
        let body = match anthropic_payload(media.as_ref(), &request).await {
            Ok(body) => body,
            Err(_) => {
                yield Err("invalid_image_attachment".to_string());
                return;
            }
        };

        let response = match client
            .post(&url)
            .header("x-api-key", &api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
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
        let mut event: Option<String> = None;
        while let Some(item) = bytes.next().await {
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(_) => {
                    yield Err(format!("{provider_name} stream failed"));
                    return;
                }
            };
            for line in take_complete_lines(&mut buffer, &chunk) {
                if let Some(name) = line.strip_prefix("event:") {
                    event = Some(name.trim().to_string());
                    continue;
                }
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                let parsed: Value = match serde_json::from_str(data) {
                    Ok(parsed) => parsed,
                    Err(_) => {
                        yield Err(format!("{provider_name} returned an unreadable response"));
                        return;
                    }
                };
                let kind = parsed
                    .get("type")
                    .and_then(Value::as_str)
                    .or(event.as_deref())
                    .unwrap_or_default();

                match kind {
                    "message_start" => {
                        let prompt_tokens = parsed
                            .get("message")
                            .and_then(|message| message.get("usage"))
                            .and_then(|usage| usage.get("input_tokens"))
                            .and_then(Value::as_u64)
                            .map(|count| count as u32);
                        yield Ok(GenerationChunk {
                            prompt_tokens,
                            source: Some(provider_name.clone()),
                            ..Default::default()
                        });
                    }
                    "content_block_delta" => {
                        let delta = parsed.get("delta");
                        let is_text = delta
                            .and_then(|value| value.get("type"))
                            .and_then(Value::as_str)
                            == Some("text_delta");
                        if is_text {
                            if let Some(text) = delta
                                .and_then(|value| value.get("text"))
                                .and_then(Value::as_str)
                            {
                                if !text.is_empty() {
                                    yield Ok(GenerationChunk {
                                        content: text.to_string(),
                                        tokens_emitted: 1,
                                        source: Some(provider_name.clone()),
                                        ..Default::default()
                                    });
                                }
                            }
                        }
                    }
                    "message_delta" => {
                        if let Some(reason) = parsed
                            .get("delta")
                            .and_then(|value| value.get("stop_reason"))
                            .and_then(Value::as_str)
                        {
                            yield Ok(GenerationChunk {
                                finish_reason: Some(reason.to_string()),
                                source: Some(provider_name.clone()),
                                ..Default::default()
                            });
                        }
                    }
                    // Ping, content block boundaries, and message_stop carry
                    // no model output in this phase.
                    _ => {}
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::messages_url;

    #[test]
    fn messages_url_does_not_duplicate_a_v1_suffix() {
        assert_eq!(
            messages_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            messages_url("https://proxy.example/v1/"),
            "https://proxy.example/v1/messages"
        );
    }
}
