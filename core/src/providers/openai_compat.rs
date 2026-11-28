//! Shared transport for providers that speak OpenAI's `/chat/completions`
//! streaming protocol (OpenAI itself and Kimi/Moonshot in sub-project A).
//!
//! The wire format is built here so both adapters send exactly the same
//! request shape. Tool definitions are deliberately not forwarded: cloud
//! routes are text-only, and the API rejects tool turns before a request ever
//! reaches this module.

use super::{bare_model, descriptor_for, CloudProvider, ProviderDescriptor, ProviderId};
use crate::api::backend::{GenerationChunk, GenerationRequest};
use futures::stream::{BoxStream, StreamExt};
use serde_json::{json, Map, Value};

/// An adapter backed by the OpenAI-compatible JSON/SSE protocol.
#[derive(Debug, Clone)]
pub(crate) struct OpenAiCompatProvider {
    id: ProviderId,
    client: reqwest::Client,
    base_url: String,
}

impl OpenAiCompatProvider {
    pub(crate) fn new(id: ProviderId, client: reqwest::Client, base_url: String) -> Self {
        Self {
            id,
            client,
            base_url,
        }
    }
}

impl CloudProvider for OpenAiCompatProvider {
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
        openai_compat_stream(
            self.client.clone(),
            &self.base_url,
            self.id,
            api_key,
            request,
        )
    }
}

/// Builds the POST body. Only fields the backend understands are included;
/// `tools` and `tool_choice` are never sent to a cloud provider.
fn payload(request: &GenerationRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".to_string(), json!(bare_model(&request.model)));
    body.insert("messages".to_string(), json!(request.messages));
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
    Value::Object(body)
}

const MAX_SSE_LINE_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub(crate) struct SseLineTooLong;

/// Drains `chunk` into `buffer` and returns every complete SSE line. A line
/// split across byte frames stays in the buffer until its `\n` arrives, so a
/// JSON object is never parsed half-written. Lines larger than
/// [`MAX_SSE_LINE_BYTES`] are rejected so an unbounded provider response cannot
/// grow the buffer without limit.
pub(crate) fn take_complete_lines(
    buffer: &mut Vec<u8>,
    chunk: &[u8],
) -> Result<Vec<String>, SseLineTooLong> {
    buffer.extend_from_slice(chunk);
    let mut lines = Vec::new();
    loop {
        let Some(position) = buffer.iter().position(|byte| *byte == b'\n') else {
            if buffer.len() > MAX_SSE_LINE_BYTES {
                return Err(SseLineTooLong);
            }
            break;
        };
        if position > MAX_SSE_LINE_BYTES {
            return Err(SseLineTooLong);
        }
        let mut raw: Vec<u8> = buffer.drain(..=position).collect();
        raw.pop();
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
        lines.push(String::from_utf8_lossy(&raw).into_owned());
    }
    Ok(lines)
}

/// Streams one OpenAI-compatible chat turn.
///
/// Errors name only the provider and (for HTTP failures) the status code. The
/// key, request body, and response body are never formatted into an error, so
/// a provider failure cannot leak a credential into logs or telemetry.
pub(crate) fn openai_compat_stream(
    client: reqwest::Client,
    base_url: &str,
    provider: ProviderId,
    api_key: &str,
    request: GenerationRequest,
) -> BoxStream<'static, Result<GenerationChunk, String>> {
    let provider_name = provider.as_str().to_string();
    let api_key = api_key.to_string();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = payload(&request);

    Box::pin(async_stream::stream! {
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
            yield Err(format!(
                "{provider_name} returned HTTP {}",
                status.as_u16()
            ));
            return;
        }

        let mut bytes = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        let mut completion_seen = false;
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
                if data == "[DONE]" {
                    completion_seen = true;
                    break;
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
                    .filter(|reason| !reason.is_null())
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
                    completion_seen = true;
                    yield Ok(GenerationChunk {
                        finish_reason: Some(finish_reason),
                        source: Some(provider_name.clone()),
                        ..Default::default()
                    });
                    break;
                }
            }
            if completion_seen {
                break;
            }
        }
        if !completion_seen {
            yield Err(format!("{provider_name} stream ended before completion"));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{take_complete_lines, MAX_SSE_LINE_BYTES};

    #[test]
    fn sse_lines_can_span_byte_frames_and_share_a_frame() {
        let mut buffer = Vec::new();
        let first = take_complete_lines(&mut buffer, b"data: {\"a\":1}\n\nda").unwrap();
        assert_eq!(first, vec!["data: {\"a\":1}".to_string(), String::new()]);

        let second =
            take_complete_lines(&mut buffer, b"ta: {\"b\":2}\n\ndata: {\"c\":3}\n\n").unwrap();
        assert_eq!(
            second,
            vec![
                "data: {\"b\":2}".to_string(),
                String::new(),
                "data: {\"c\":3}".to_string(),
                String::new(),
            ]
        );
        assert!(buffer.is_empty(), "no partial line is left behind");
    }

    #[test]
    fn crlf_and_partial_lines_are_handled() {
        let mut buffer = Vec::new();
        assert!(take_complete_lines(&mut buffer, b"data: hel")
            .unwrap()
            .is_empty());
        assert_eq!(
            take_complete_lines(&mut buffer, b"lo\r\n").unwrap(),
            vec!["data: hello".to_string()]
        );
    }

    #[test]
    fn oversized_sse_lines_are_rejected() {
        let mut buffer = Vec::new();
        let oversized = vec![b'x'; MAX_SSE_LINE_BYTES + 1];
        assert!(take_complete_lines(&mut buffer, &oversized).is_err());

        let mut buffer = Vec::new();
        let mut oversized = vec![b'x'; MAX_SSE_LINE_BYTES + 1];
        oversized.push(b'\n');
        assert!(take_complete_lines(&mut buffer, &oversized).is_err());
    }
}
