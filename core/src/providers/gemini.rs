//! Google Gemini adapter using the `streamGenerateContent` SSE endpoint.
//!
//! Gemini authenticates with an `x-goog-api-key` header, never a query-string
//! key, so credentials stay out of URLs and logs. System instructions are
//! folded into the first user part because Gemini's `contents` array accepts
//! only `user` and `model` roles. Image bytes are resolved only through the
//! shared media store and are passed to the committed message mapper as raw
//! base64.

use super::messages::to_gemini_parts;
use super::openai_compat::take_complete_lines;
use super::{bare_model, descriptor_for, CloudProvider, ProviderDescriptor, ProviderId};
use crate::api::backend::{
    ChatMessage, ContentPart, GenerationChunk, GenerationRequest, MessageContent, Stop,
};
use crate::media::{MediaError, MediaStore};
use futures::stream::{BoxStream, StreamExt};
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// Adapter backed by Gemini's `streamGenerateContent` endpoint.
#[derive(Clone)]
pub(crate) struct GeminiProvider {
    client: reqwest::Client,
    base_url: String,
    media: Option<Arc<MediaStore>>,
}

impl GeminiProvider {
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
) -> GeminiProvider {
    GeminiProvider::new(client, base_url, media)
}

impl CloudProvider for GeminiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Gemini
    }

    fn descriptor(&self) -> &'static ProviderDescriptor {
        descriptor_for(ProviderId::Gemini)
    }

    fn chat_stream(
        &self,
        request: GenerationRequest,
        api_key: &str,
    ) -> BoxStream<'static, Result<GenerationChunk, String>> {
        gemini_stream(
            self.client.clone(),
            &self.base_url,
            api_key,
            request,
            self.media.clone(),
        )
    }
}

/// Maps one message to Gemini parts. Each image gets its own inline block so a
/// multi-image turn cannot reuse the first image's payload.
async fn mapped_parts(
    media: Option<&Arc<MediaStore>>,
    message: &ChatMessage,
) -> Result<Vec<Value>, MediaError> {
    if !message.content.has_images() {
        return Ok(to_gemini_parts(message, ""));
    }
    let MessageContent::Parts(parts) = &message.content else {
        return Ok(to_gemini_parts(message, ""));
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
        out.extend(to_gemini_parts(&single, &base64_data));
    }
    Ok(out)
}

/// Builds the request body. Gemini roles are `user` and `model`; system turns
/// are concatenated and prefixed to the first user part so no instruction is
/// lost. Tool schemas are deliberately not forwarded (spec D-A2).
async fn gemini_payload(
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

    let mut contents: Vec<Value> = Vec::new();
    let mut system_prefixed = system.is_empty();
    for message in request
        .messages
        .iter()
        .filter(|message| message.role != "system")
    {
        let role = if message.role == "assistant" {
            "model"
        } else {
            "user"
        };
        let mut parts = mapped_parts(media, message).await?;
        if !system_prefixed && role == "user" {
            if let Some(text_part) = parts.iter_mut().find(|part| part.get("text").is_some()) {
                let text = text_part["text"].as_str().unwrap_or_default().to_string();
                text_part["text"] = json!(format!("{system}\n{text}"));
            } else {
                parts.insert(0, json!({ "text": system }));
            }
            system_prefixed = true;
        }
        contents.push(json!({
            "role": role,
            "parts": parts,
        }));
    }

    let mut config = Map::new();
    config.insert("temperature".to_string(), json!(request.temperature));
    config.insert("maxOutputTokens".to_string(), json!(request.max_tokens));
    if let Some(stop) = &request.stop {
        let sequences = match stop {
            Stop::One(value) => vec![value.clone()],
            Stop::Many(values) => values.clone(),
        };
        config.insert("stopSequences".to_string(), json!(sequences));
    }

    Ok(json!({
        "contents": contents,
        "generationConfig": Value::Object(config),
    }))
}

fn stream_url(base_url: &str, model: &str) -> String {
    format!(
        "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
        base_url.trim_end_matches('/'),
        model
    )
}

fn gemini_stream(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
    request: GenerationRequest,
    media: Option<Arc<MediaStore>>,
) -> BoxStream<'static, Result<GenerationChunk, String>> {
    let provider_name = ProviderId::Gemini.as_str().to_string();
    let api_key = api_key.to_string();
    let url = stream_url(base_url, bare_model(&request.model));

    Box::pin(async_stream::stream! {
        let body = match gemini_payload(media.as_ref(), &request).await {
            Ok(body) => body,
            Err(_) => {
                yield Err("invalid_image_attachment".to_string());
                return;
            }
        };

        let response = match client
            .post(&url)
            .header("x-goog-api-key", &api_key)
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
        let mut estimated_text_chunks: u32 = 0;
        let mut authoritative_tokens: Option<u32> = None;
        while let Some(item) = bytes.next().await {
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(_) => {
                    yield Err(format!("{provider_name} stream failed"));
                    return;
                }
            };
            let lines = match take_complete_lines(&mut buffer, &chunk) {
                Ok(lines) => lines,
                Err(_) => {
                    yield Err(format!("{provider_name} returned an oversized response line"));
                    return;
                }
            };
            for line in lines {
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

                let mut text = String::new();
                let mut finish_reason: Option<String> = None;
                if let Some(candidates) = parsed.get("candidates").and_then(Value::as_array) {
                    if let Some(candidate) = candidates.first() {
                        if let Some(parts) = candidate
                            .get("content")
                            .and_then(|content| content.get("parts"))
                            .and_then(Value::as_array)
                        {
                            for part in parts {
                                if let Some(piece) = part.get("text").and_then(Value::as_str) {
                                    text.push_str(piece);
                                }
                            }
                        }
                        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                            finish_reason = Some(reason.to_ascii_lowercase());
                        }
                    }
                }

                if !text.is_empty() {
                    let usage_seen = authoritative_tokens.is_some();
                    if !usage_seen {
                        estimated_text_chunks = estimated_text_chunks.saturating_add(1);
                    }
                    yield Ok(GenerationChunk {
                        content: text,
                        tokens_emitted: if usage_seen { 0 } else { 1 },
                        source: Some(provider_name.clone()),
                        ..Default::default()
                    });
                }

                if let Some(usage) = parsed.get("usageMetadata") {
                    let prompt_tokens = usage
                        .get("promptTokenCount")
                        .and_then(Value::as_u64)
                        .map(|count| count as u32);
                    let completion_tokens = usage
                        .get("candidatesTokenCount")
                        .and_then(Value::as_u64)
                        .map(|count| count as u32)
                        .unwrap_or(0);
                    let correction = completion_tokens.saturating_sub(
                        authoritative_tokens.unwrap_or(estimated_text_chunks),
                    );
                    authoritative_tokens = Some(completion_tokens);
                    yield Ok(GenerationChunk {
                        prompt_tokens,
                        tokens_emitted: correction,
                        finish_reason: Some(
                            finish_reason.unwrap_or_else(|| "stop".to_string()),
                        ),
                        source: Some(provider_name.clone()),
                        ..Default::default()
                    });
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::stream_url;

    #[test]
    fn stream_url_keeps_the_key_out_of_the_query() {
        let url = stream_url(
            "https://generativelanguage.googleapis.com/",
            "gemini-2.5-flash",
        );
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
        assert!(!url.contains("key="));
    }
}
