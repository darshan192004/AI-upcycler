//! Google Gemini adapter using the `streamGenerateContent` SSE endpoint.
//!
//! Gemini authenticates with an `x-goog-api-key` header, never a query-string
//! key, so credentials stay out of URLs and logs. System instructions are
//! folded into the first user part because Gemini's `contents` array accepts
//! only `user` and `model` roles.

use super::openai_compat::take_complete_lines;
use super::{bare_model, descriptor_for, CloudProvider, ProviderDescriptor, ProviderId};
use crate::api::backend::{GenerationChunk, GenerationRequest, Stop};
use futures::stream::{BoxStream, StreamExt};
use serde_json::{json, Map, Value};

/// Adapter backed by Gemini's `streamGenerateContent` endpoint.
#[derive(Debug, Clone)]
pub(crate) struct GeminiProvider {
    client: reqwest::Client,
    base_url: String,
}

impl GeminiProvider {
    pub(crate) fn new(client: reqwest::Client, base_url: String) -> Self {
        Self { client, base_url }
    }
}

pub(crate) fn provider(client: reqwest::Client, base_url: String) -> GeminiProvider {
    GeminiProvider::new(client, base_url)
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
        gemini_stream(self.client.clone(), &self.base_url, api_key, request)
    }
}

/// Builds the request body. Gemini roles are `user` and `model`; system turns
/// are concatenated and prefixed to the first user part so no instruction is
/// lost. Tool schemas are deliberately not forwarded (spec D-A2).
fn payload(request: &GenerationRequest) -> Value {
    let system = request
        .messages
        .iter()
        .filter(|message| message.role == "system")
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let mut contents: Vec<Value> = Vec::new();
    for (index, message) in request
        .messages
        .iter()
        .filter(|message| message.role != "system")
        .enumerate()
    {
        let role = if message.role == "assistant" {
            "model"
        } else {
            "user"
        };
        let mut text = message.content.clone();
        if index == 0 && role == "user" && !system.is_empty() {
            text = format!("{system}\n{text}");
        }
        contents.push(json!({
            "role": role,
            "parts": [{ "text": text }],
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

    json!({
        "contents": contents,
        "generationConfig": Value::Object(config),
    })
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
) -> BoxStream<'static, Result<GenerationChunk, String>> {
    let provider_name = ProviderId::Gemini.as_str().to_string();
    let api_key = api_key.to_string();
    let url = stream_url(base_url, bare_model(&request.model));
    let body = payload(&request);

    Box::pin(async_stream::stream! {
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
                    yield Ok(GenerationChunk {
                        content: text,
                        tokens_emitted: 1,
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
                    yield Ok(GenerationChunk {
                        prompt_tokens,
                        tokens_emitted: completion_tokens,
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
