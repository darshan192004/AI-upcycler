//! Token-generation backend abstraction.
//!
//! The orchestrator talks to inference through this trait so the transport
//! (local `llama.cpp`, pipelined cluster workers, a future GPU path) can be
//! swapped without touching the HTTP layer. Until a real engine is wired, the
//! [`StubBackend`] provides a deterministic, clearly-labelled placeholder.

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenerationRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: u32,
    /// The chat session id (the API's session namespace). The backend keys
    /// worker KV session ids off it; llama-server ignores it.
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenerationChunk {
    pub content: String,
    pub tokens_emitted: u32,
    /// The TRUE count of prompt tokens (the prefill length) set by backends
    /// that encode the prompt themselves. `None` leaves the API's estimate
    /// (`approx_tokens`) in `usage` - the llama fallback has no token-level
    /// view of its JSON streaming.
    pub prompt_tokens: Option<u32>,
}

/// Produces a stream of token chunks for a chat request.
pub trait GenerationBackend: Send + Sync + 'static {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> BoxStream<'static, Result<GenerationChunk, String>>;
}

/// Deterministic offline stand-in for a real inference engine.
///
/// It echoes a fixed acknowledgement split into word-sized chunks so streaming,
/// persistence and telemetry paths can be exercised end-to-end. It performs no
/// inference; replace it with a real backend before claiming model output.
#[derive(Debug, Default, Clone)]
pub struct StubBackend;

impl StubBackend {
    pub fn new() -> Self {
        Self
    }

    /// Builds the canned reply for a request. Deterministic for a given input.
    pub fn reply(request: &GenerationRequest) -> String {
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let prompt = truncate(last_user, 80);
        format!(
            "[stub:{model}] No inference engine is loaded yet. Received {count} message(s); \
             last prompt: \"{prompt}\". This is a placeholder response from the AI Upcycler core.",
            model = request.model,
            count = request.messages.len(),
        )
    }
}

impl GenerationBackend for StubBackend {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> BoxStream<'static, Result<GenerationChunk, String>> {
        let reply = Self::reply(&request);
        Box::pin(async_stream::stream! {
            for (index, word) in reply.split_inclusive(' ').enumerate() {
                if index as u32 >= request.max_tokens.max(1) {
                    break;
                }
                yield Ok(GenerationChunk {
                    content: word.to_string(),
                    tokens_emitted: 1,
                    prompt_tokens: None,
                });
            }
        })
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn request() -> GenerationRequest {
        GenerationRequest {
            model: "llama-8b".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "What is AI Upcycler?".into(),
            }],
            temperature: 0.7,
            max_tokens: 512,
            session_id: "s-test".into(),
        }
    }

    #[test]
    fn reply_is_deterministic_and_labels_itself() {
        let reply = StubBackend::reply(&request());
        assert!(reply.contains("[stub:llama-8b]"));
        assert!(reply.contains("What is AI Upcycler?"));
        assert_eq!(reply, StubBackend::reply(&request()));
    }

    #[test]
    fn reply_truncates_long_prompts() {
        let mut req = request();
        req.messages[0].content = "x".repeat(200);
        let reply = StubBackend::reply(&req);
        assert!(reply.contains('…'));
        assert!(!reply.contains(&"x".repeat(120)));
    }

    #[tokio::test]
    async fn generate_streams_chunks_that_reassemble_the_reply() {
        let backend = StubBackend::new();
        let expected = StubBackend::reply(&request());
        let chunks: Vec<_> = backend
            .generate(request())
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|c| c.expect("chunk ok"))
            .collect();

        assert!(!chunks.is_empty());
        let reassembled: String = chunks.iter().map(|c| c.content.as_str()).collect();
        assert_eq!(reassembled, expected);
        assert!(chunks.iter().all(|c| c.tokens_emitted == 1));
    }

    #[tokio::test]
    async fn generate_respects_max_tokens() {
        let mut req = request();
        req.max_tokens = 3;
        let count = StubBackend::new().generate(req).count().await;
        assert_eq!(count, 3);
    }
}
