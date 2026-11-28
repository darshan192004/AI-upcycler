//! Token-generation backend abstraction.
//!
//! The orchestrator talks to inference through this trait so the transport
//! (local `llama.cpp`, pipelined cluster workers, a future GPU path) can be
//! swapped without touching the HTTP layer. Until a real engine is wired, the
//! [`StubBackend`] provides a deterministic, clearly-labelled placeholder.

use futures::stream::BoxStream;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single typed piece of chat content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Image {
        media_hash: String,
        mime_type: String,
        file_name: String,
        byte_count: u64,
    },
}

/// Chat content is either plain text (the legacy shape) or a list of typed
/// parts. `Parts` is declared first so serde's untagged deserializer tries the
/// array shape first; a bare JSON string still lands in `Text`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum MessageContent {
    Parts(Vec<ContentPart>),
    Text(String),
}

impl Default for MessageContent {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl From<String> for MessageContent {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for MessageContent {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl std::fmt::Display for MessageContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.content_text())
    }
}

impl MessageContent {
    /// Concatenated text of every `Text` part, or the plain text unchanged.
    pub fn content_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Parts(parts) => parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// Borrowed text for text-only messages; image parts contribute nothing.
    /// Consumers that cannot render images use this to keep their legacy shape.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Text(text) => text.as_str(),
            Self::Parts(parts) => parts
                .iter()
                .find_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::Image { .. } => None,
                })
                .unwrap_or(""),
        }
    }

    /// True when the message carries at least one image part.
    pub fn has_images(&self) -> bool {
        match self {
            Self::Text(_) => false,
            Self::Parts(parts) => parts
                .iter()
                .any(|part| matches!(part, ContentPart::Image { .. })),
        }
    }
}

/// One OpenAI chat message.
///
/// Agents send `null` or omit `content` on assistant tool-call turns; both
/// become `MessageContent::Text(String::new())` (spec D-A3). The tool fields
/// are optional and omitted from serialization when absent, so a plain chat
/// message still goes on the wire exactly as before.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub content: MessageContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

fn null_as_empty<'de, D>(deserializer: D) -> Result<MessageContent, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<MessageContent>::deserialize(deserializer)?.unwrap_or_default())
}

/// A complete tool call, as an assistant turn carries it and as the buffered
/// response returns it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionCall {
    pub name: String,
    /// The arguments are a JSON-encoded **string**, not an object: that is
    /// what the OpenAI schema specifies and what agents parse.
    #[serde(default)]
    pub arguments: String,
}

/// One streamed tool-call fragment. llama-server sends the id/type/name once
/// and the argument JSON in pieces; the API forwards the pieces and the
/// buffered path merges them by `index`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolCallDelta {
    #[serde(default)]
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCallDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FunctionCallDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

/// A tool definition supplied by the client, passed through to the engine
/// unchanged: the model's chat template, not this server, interprets it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

fn function_kind() -> String {
    "function".into()
}

/// OpenAI's `stop` accepts one string or an array of them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Stop {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
pub struct GenerationRequest {
    #[serde(default)]
    pub model: String,
    /// Explicit provider override from an agent request. Legacy clients omit
    /// it, so it defaults to `None` and normal routing picks a provider.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub temperature: f32,
    #[serde(default)]
    pub max_tokens: u32,
    /// The chat session id (the API's session namespace). The backend keys
    /// worker KV session ids off it; llama-server ignores it.
    #[serde(default)]
    pub session_id: String,
    /// Function tools from the client. Only a host engine can render them;
    /// the API refuses a tool request when no such engine exists (spec D-A2).
    #[serde(default)]
    pub tools: Option<Vec<ToolDef>>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub stop: Option<Stop>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub seed: Option<u64>,
}

impl GenerationRequest {
    /// True when the client asked for at least one tool. This is the routing
    /// fact: such a request must never reach the cluster (spec D-A2).
    pub fn wants_tools(&self) -> bool {
        self.tools.as_ref().is_some_and(|tools| !tools.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GenerationChunk {
    pub content: String,
    pub tokens_emitted: u32,
    /// The TRUE count of prompt tokens (the prefill length) set by backends
    /// that encode the prompt themselves. `None` leaves the API's estimate
    /// (`approx_tokens`) in `usage` - the llama fallback has no token-level
    /// view of its JSON streaming.
    pub prompt_tokens: Option<u32>,
    /// Tool-call fragments from the engine, in the order they arrived.
    pub tool_calls: Option<Vec<ToolCallDelta>>,
    /// The engine's finish reason, when it sent one. The API defaults to
    /// `"stop"`, or `"tool_calls"` when the turn produced calls.
    pub finish_reason: Option<String>,
    /// Where this chunk came from: `local`, `adb:<serial>`, or a provider id.
    /// `None` is legacy/unknown. Every real backend labels the chunks it emits
    /// so the metrics layer can report the execution source.
    pub source: Option<String>,
}

/// Produces a stream of token chunks for a chat request.
pub trait GenerationBackend: Send + Sync + 'static {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> BoxStream<'static, Result<GenerationChunk, String>>;

    /// Whether this backend can render tool definitions and return calls.
    ///
    /// Only a real host engine can. The cluster path samples ChatML with
    /// system/user/assistant turns and has no tool renderer, so pretending
    /// otherwise would return text that looks like a call but is not one.
    fn tool_calling_available(&self) -> bool {
        false
    }

    /// True when this backend can actually produce model output. The labelled
    /// offline stub is false so `Auto` can fall through to a cloud provider.
    fn is_real(&self) -> bool {
        true
    }
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

    /// Minimal echo reply used by the inline compatibility tests. Image
    /// attachments are refused because there is no vision engine here.
    pub fn reply(&self, request: &GenerationRequest) -> Result<ChatMessage, String> {
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let text = match &last_user {
            MessageContent::Text(text) => text.clone(),
            MessageContent::Parts(_) => {
                return Err("image_attachments_require_vision_provider".into())
            }
        };
        Ok(ChatMessage {
            role: "assistant".into(),
            content: MessageContent::Text(text),
            ..Default::default()
        })
    }

    /// Deterministic, clearly-labelled canned text for the streaming path.
    fn canned_text(request: &GenerationRequest) -> Result<String, String> {
        let last_user = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let text = last_user.as_str();
        if last_user.has_images() {
            return Err("image_attachments_require_vision_provider".into());
        }
        let prompt = truncate(text, 80);
        Ok(format!(
            "[stub:{model}] No inference engine is loaded yet. Received {count} message(s); \
             last prompt: \"{prompt}\". This is a placeholder response from the AI Upcycler core.",
            model = request.model,
            count = request.messages.len(),
        ))
    }
}

impl GenerationBackend for StubBackend {
    fn is_real(&self) -> bool {
        false
    }

    fn generate(
        &self,
        request: GenerationRequest,
    ) -> BoxStream<'static, Result<GenerationChunk, String>> {
        let reply = Self::canned_text(&request);
        Box::pin(async_stream::stream! {
            let reply = match reply {
                Ok(reply) => reply,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };
            for (index, word) in reply.split_inclusive(' ').enumerate() {
                if index as u32 >= request.max_tokens.max(1) {
                    break;
                }
                yield Ok(GenerationChunk {
                    content: word.to_string(),
                    tokens_emitted: 1,
                    source: Some("local".into()),
                    ..Default::default()
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
                ..Default::default()
            }],
            temperature: 0.7,
            max_tokens: 512,
            session_id: "s-test".into(),
            ..Default::default()
        }
    }

    #[test]
    fn canned_text_is_deterministic_and_labels_itself() {
        let reply = StubBackend::canned_text(&request()).expect("text request");
        assert!(reply.contains("[stub:llama-8b]"));
        assert!(reply.contains("What is AI Upcycler?"));
        assert_eq!(
            reply,
            StubBackend::canned_text(&request()).expect("text request")
        );
    }

    #[test]
    fn canned_text_truncates_long_prompts() {
        let mut req = request();
        req.messages[0].content = MessageContent::Text("x".repeat(200));
        let reply = StubBackend::canned_text(&req).expect("text request");
        assert!(reply.contains('…'));
        assert!(!reply.contains(&"x".repeat(120)));
    }

    #[tokio::test]
    async fn generate_streams_chunks_that_reassemble_the_reply() {
        let backend = StubBackend::new();
        let expected = StubBackend::canned_text(&request()).expect("text request");
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

    #[test]
    fn null_or_missing_content_becomes_empty() {
        let null: ChatMessage = serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": null
        }))
        .expect("null content is legal");
        assert_eq!(null.content, MessageContent::Text(String::new()));
        assert!(null.tool_calls.is_none());

        let missing: ChatMessage =
            serde_json::from_value(serde_json::json!({ "role": "assistant" }))
                .expect("missing content is legal");
        assert_eq!(missing.content, MessageContent::Text(String::new()));
    }

    #[test]
    fn an_assistant_tool_call_turn_round_trips() {
        let message: ChatMessage = serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": { "name": "read", "arguments": "{\"path\":\"a.txt\"}" }
            }]
        }))
        .expect("tool-call turn");
        let calls = message.tool_calls.as_ref().expect("calls");
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].kind, "function");
        assert_eq!(calls[0].function.name, "read");
        assert_eq!(calls[0].function.arguments, "{\"path\":\"a.txt\"}");

        let wire = serde_json::to_value(&message).expect("serialize");
        assert_eq!(wire["tool_calls"][0]["function"]["name"], "read");
        assert!(wire.get("tool_call_id").is_none());
    }

    #[test]
    fn a_tool_result_turn_keeps_its_call_id() {
        let message: ChatMessage = serde_json::from_value(serde_json::json!({
            "role": "tool",
            "content": "file contents",
            "tool_call_id": "call_1"
        }))
        .expect("tool turn");
        assert_eq!(message.tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn tool_definitions_are_passed_through() {
        let def: ToolDef = serde_json::from_value(serde_json::json!({
            "type": "function",
            "function": {
                "name": "read",
                "description": "Read a file",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        }))
        .expect("tool definition");
        assert_eq!(def.kind, "function");
        assert_eq!(def.function.name, "read");
        assert_eq!(
            def.function.parameters.as_ref().unwrap()["required"][0],
            "path"
        );
    }

    #[test]
    fn wants_tools_only_when_the_array_is_not_empty() {
        let mut req = request();
        assert!(!req.wants_tools());
        req.tools = Some(vec![]);
        assert!(!req.wants_tools());
        req.tools = Some(vec![ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "read".into(),
                description: None,
                parameters: None,
            },
        }]);
        assert!(req.wants_tools());
    }

    #[test]
    fn stub_backend_is_not_a_real_local_engine() {
        assert!(!StubBackend::new().is_real());
    }

    #[test]
    fn chunks_can_carry_their_execution_source() {
        let chunk = GenerationChunk {
            source: Some("adb:192.168.1.9:5555".into()),
            ..Default::default()
        };
        assert_eq!(chunk.source.as_deref(), Some("adb:192.168.1.9:5555"));
        assert_eq!(GenerationChunk::default().source, None);
    }

    // --- MessageContent migration tests (plan Task 2) -----------------------

    fn req_from_content(content: Value) -> GenerationRequest {
        serde_json::from_value(serde_json::json!({
            "messages":[{"role":"user","content":content}],
            "max_tokens":64
        }))
        .unwrap()
    }

    #[test]
    fn stub_backend_echoes_text_content() {
        let out = StubBackend
            .reply(&req_from_content(serde_json::json!("ping")))
            .unwrap();
        assert_eq!(out.content, MessageContent::Text("ping".into()));
    }

    #[test]
    fn stub_backend_rejects_image_parts() {
        let req = req_from_content(serde_json::json!([{
            "type":"image",
            "media_hash":"ab".repeat(32),
            "mime_type":"image/png",
            "file_name":"a.png",
            "byte_count":4
        }]));
        assert_eq!(
            StubBackend.reply(&req),
            Err("image_attachments_require_vision_provider".into())
        );
    }
}
