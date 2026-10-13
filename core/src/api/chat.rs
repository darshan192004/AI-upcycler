//! `POST /v1/chat/completions` — OpenAI-compatible chat, streaming or buffered.
//!
//! Both modes persist the user turn and the assistant reply to the SQLite store
//! and publish `generation_metrics` telemetry while tokens flow.

use std::convert::Infallible;
use std::time::Instant;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::api::backend::{ChatMessage, GenerationRequest};
use crate::api::{ApiError, ApiResult, AppState};
use crate::discovery::mdns::NodeRegistry;
use crate::store::{NewChatMessage, Store};
use crate::telemetry::{
    now_epoch_secs, GenerationMetrics, TelemetryBus, TelemetryEvent, WorkerTelemetry,
};

const ROLE_ASSISTANT: &str = "assistant";

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub session_id: Option<String>,
}

fn default_temperature() -> f32 {
    0.7
}

fn default_max_tokens() -> u32 {
    1024
}

#[derive(Debug, Serialize)]
struct ChatCompletionResponse {
    id: String,
    object: &'static str,
    created: u64,
    model: String,
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Debug, Serialize)]
struct Choice {
    index: u32,
    message: ChatMessage,
    finish_reason: &'static str,
}

#[derive(Debug, Serialize)]
struct Usage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

pub async fn chat_completions(
    State(state): State<AppState>,
    Json(request): Json<ChatCompletionRequest>,
) -> ApiResult<Response> {
    if request.messages.is_empty() {
        return Err(ApiError::BadRequest("messages must not be empty".into()));
    }
    let model = state
        .store
        .get_model(&request.model)?
        .ok_or_else(|| ApiError::NotFound(format!("model '{}'", request.model)))?;

    let session_id = resolve_session(&state, &request)?;
    if let Some(user) = request.messages.iter().rev().find(|m| m.role == "user") {
        state.store.append_message(&NewChatMessage {
            session_id: session_id.clone(),
            role: user.role.clone(),
            content: user.content.clone(),
            token_count: approx_tokens(&user.content),
            generation_time_sec: 0.0,
        })?;
    }

    let generation = GenerationRequest {
        model: model.model_id,
        messages: request.messages.clone(),
        temperature: request.temperature,
        max_tokens: request.max_tokens,
    };

    if request.stream {
        Ok(stream_response(state, session_id, generation).into_response())
    } else {
        Ok(buffered_response(state, session_id, generation)
            .await?
            .into_response())
    }
}

async fn buffered_response(
    state: AppState,
    session_id: String,
    generation: GenerationRequest,
) -> ApiResult<Json<ChatCompletionResponse>> {
    let started = Instant::now();
    let mut stream = state.backend.generate(generation.clone());

    let mut content = String::new();
    let mut completion_tokens: u64 = 0;
    while let Some(item) = stream.next().await {
        let chunk = item.map_err(ApiError::Internal)?;
        content.push_str(&chunk.content);
        completion_tokens += u64::from(chunk.tokens_emitted);
    }
    let elapsed = started.elapsed().as_secs_f64();

    persist_assistant(
        &state.store,
        &session_id,
        &content,
        completion_tokens,
        elapsed,
    );
    publish_metrics(
        &state.telemetry,
        &state.registry,
        completion_tokens,
        elapsed,
    );

    let prompt_tokens = generation
        .messages
        .iter()
        .map(|m| approx_tokens(&m.content))
        .sum::<i64>() as u64;

    Ok(Json(ChatCompletionResponse {
        id: chatcmpl_id(),
        object: "chat.completion",
        created: now_epoch_secs(),
        model: generation.model,
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: ROLE_ASSISTANT.into(),
                content,
            },
            finish_reason: "stop",
        }],
        usage: Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
    }))
}

fn stream_response(
    state: AppState,
    session_id: String,
    generation: GenerationRequest,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let id = chatcmpl_id();
    let model = generation.model.clone();
    let store = state.store.clone();
    let telemetry = state.telemetry.clone();
    let registry = state.registry.clone();

    let stream = async_stream::stream! {
        let started = Instant::now();
        yield Ok(sse_event(chunk_value(&id, &model, json!({ "role": ROLE_ASSISTANT }), None)));

        let mut content = String::new();
        let mut completion_tokens: u64 = 0;
        let mut backend = state.backend.generate(generation.clone());
        while let Some(item) = backend.next().await {
            match item {
                Ok(chunk) => {
                    completion_tokens += u64::from(chunk.tokens_emitted);
                    content.push_str(&chunk.content);
                    publish_metrics(&telemetry, &registry, completion_tokens, started.elapsed().as_secs_f64());
                    yield Ok(sse_event(chunk_value(&id, &model, json!({ "content": chunk.content }), None)));
                }
                Err(error) => {
                    yield Ok(Event::default().data(json!({ "error": { "message": error } }).to_string()));
                    break;
                }
            }
        }

        yield Ok(sse_event(chunk_value(&id, &model, json!({}), Some("stop"))));
        yield Ok(Event::default().data("[DONE]"));

        persist_assistant(&store, &session_id, &content, completion_tokens, started.elapsed().as_secs_f64());
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn resolve_session(state: &AppState, request: &ChatCompletionRequest) -> ApiResult<String> {
    let title = request
        .messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| truncate(&m.content, 48))
        .unwrap_or_else(|| "New chat".to_string());

    match &request.session_id {
        Some(id) => {
            if state.store.get_session(id)?.is_none() {
                state.store.create_session(id, &title, &request.model)?;
            }
            Ok(id.clone())
        }
        None => {
            let id = format!("sess-{}", Uuid::new_v4());
            state.store.create_session(&id, &title, &request.model)?;
            Ok(id)
        }
    }
}

fn persist_assistant(
    store: &Store,
    session_id: &str,
    content: &str,
    tokens: u64,
    elapsed_secs: f64,
) {
    if let Err(error) = store.append_message(&NewChatMessage {
        session_id: session_id.to_string(),
        role: ROLE_ASSISTANT.to_string(),
        content: content.to_string(),
        token_count: tokens as i64,
        generation_time_sec: elapsed_secs,
    }) {
        tracing::warn!("failed to persist assistant message: {error}");
    }
}

fn publish_metrics(
    telemetry: &TelemetryBus,
    registry: &NodeRegistry,
    current_token: u64,
    elapsed_secs: f64,
) {
    let tokens_per_second = if elapsed_secs > 0.0 {
        current_token as f64 / elapsed_secs
    } else {
        0.0
    };
    let worker_nodes = registry
        .list_nodes()
        .iter()
        .map(|node| WorkerTelemetry {
            id: node.node_id.clone(),
            temp_celsius: 0.0,
            battery_pct: 0.0,
        })
        .collect();
    telemetry.publish(TelemetryEvent::generation_metrics(GenerationMetrics {
        tokens_per_second,
        current_token,
        master_ram_utilization_pct: 0.0,
        worker_nodes,
    }));
}

fn chunk_value(
    id: &str,
    model: &str,
    delta: serde_json::Value,
    finish_reason: Option<&str>,
) -> serde_json::Value {
    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": now_epoch_secs(),
        "model": model,
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }],
    })
}

fn sse_event(value: serde_json::Value) -> Event {
    Event::default().data(value.to_string())
}

fn chatcmpl_id() -> String {
    format!("chatcmpl-{}", Uuid::new_v4())
}

fn approx_tokens(text: &str) -> i64 {
    text.split_whitespace().count().max(1) as i64
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NewModel, Store};
    use std::sync::Arc;

    fn seeded_state() -> AppState {
        let store = Arc::new(Store::open_in_memory().expect("store"));
        store
            .upsert_model(&NewModel {
                model_id: "llama-8b".into(),
                file_name: "llama.gguf".into(),
                quantization_format: "Q4_K_M".into(),
                total_layers: 32,
                weight_size_bytes: 1,
                local_file_path: "/llama.gguf".into(),
            })
            .unwrap();
        AppState::bootstrap(
            "test-cluster".into(),
            Arc::new(NodeRegistry::new()),
            store,
            TelemetryBus::new(8),
        )
    }

    fn request(stream: bool) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "llama-8b".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hello there".into(),
            }],
            temperature: 0.7,
            max_tokens: 64,
            stream,
            session_id: Some("s1".into()),
        }
    }

    #[test]
    fn chunk_values_carry_openai_shape() {
        let value = chunk_value("chatcmpl-1", "llama-8b", json!({ "content": "hi" }), None);
        assert_eq!(value["object"], "chat.completion.chunk");
        assert_eq!(value["choices"][0]["delta"]["content"], "hi");
        assert!(value["choices"][0]["finish_reason"].is_null());
    }

    #[test]
    fn session_title_uses_first_user_message() {
        let state = seeded_state();
        let id = resolve_session(&state, &request(false)).unwrap();
        let session = state.store.get_session(&id).unwrap().unwrap();
        assert_eq!(session.title, "hello there");
        assert_eq!(session.model_id, "llama-8b");
    }

    #[tokio::test]
    async fn unknown_model_is_rejected_before_generation() {
        use crate::api::ApiError;
        use axum::extract::State;
        use axum::Json;

        let state = seeded_state();
        let mut req = request(false);
        req.model = "ghost".into();
        let result = chat_completions(State(state), Json(req)).await;
        assert!(matches!(result, Err(ApiError::NotFound(_))));
    }

    #[tokio::test]
    async fn buffered_response_persists_turn_and_usage() {
        let state = seeded_state();
        let session_id = resolve_session(&state, &request(false)).unwrap();
        state
            .store
            .append_message(&NewChatMessage {
                session_id: session_id.clone(),
                role: "user".into(),
                content: "hello there".into(),
                token_count: 2,
                generation_time_sec: 0.0,
            })
            .unwrap();

        let generation = GenerationRequest {
            model: "llama-8b".into(),
            messages: request(false).messages,
            temperature: 0.7,
            max_tokens: 64,
        };
        let Json(response) = buffered_response(state.clone(), session_id.clone(), generation)
            .await
            .unwrap();

        assert_eq!(response.choices[0].finish_reason, "stop");
        assert!(response.choices[0]
            .message
            .content
            .contains("[stub:llama-8b]"));
        assert!(response.usage.completion_tokens > 0);

        let messages = state.store.list_messages(&session_id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, ROLE_ASSISTANT);
    }

    #[test]
    fn approx_tokens_counts_words() {
        assert_eq!(approx_tokens("a b c"), 3);
        assert_eq!(approx_tokens(""), 1);
    }
}
