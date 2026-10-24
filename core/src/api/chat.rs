//! `POST /v1/chat/completions` — OpenAI-compatible chat, streaming or buffered.
//!
//! Both modes persist the user turn and the assistant reply to the SQLite store
//! and publish `generation_metrics` telemetry while tokens flow.

use std::collections::{BTreeMap, HashSet};
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

use crate::api::backend::{
    ChatMessage, FunctionCall, GenerationRequest, Stop, ToolCall, ToolCallDelta, ToolDef,
};
use crate::api::{ApiError, ApiResult, AppState};
use crate::discovery::mdns::NodeRegistry;
use crate::store::{NewChatMessage, Store};
use crate::telemetry::{
    now_epoch_secs, GenerationMetrics, TelemetryBus, TelemetryEvent, WorkerTelemetry,
};
use crate::worker_health::TelemetryCache;

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
    #[serde(default)]
    pub tools: Option<Vec<ToolDef>>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(default)]
    pub stop: Option<Stop>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub seed: Option<u64>,
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
    finish_reason: String,
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

    // Agent requests are host-only in phase 1 (spec D-A2). Refuse before the
    // session or user turn is persisted, so a rejected request leaves no
    // half-written conversation behind.
    if request
        .tools
        .as_ref()
        .is_some_and(|tools| !tools.is_empty())
        && !state.backend.tool_calling_available()
    {
        return Err(ApiError::BadRequest(
            "tool calls require a local host engine (llama-server); \
             the distributed route cannot call tools"
                .into(),
        ));
    }

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
        model: model.model_id.clone(),
        messages: request.messages.clone(),
        temperature: request.temperature,
        max_tokens: request.max_tokens,
        session_id: session_id.clone(),
        tools: request.tools.clone(),
        tool_choice: request.tool_choice.clone(),
        stop: request.stop.clone(),
        top_p: request.top_p,
        seed: request.seed,
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
    // The backend that encodes the prompt itself reports the TRUE prefill
    // token count on its chunks (D5); the approximation stays the fallback.
    let mut prompt_tokens: Option<u64> = None;
    let mut tool_deltas: Vec<ToolCallDelta> = Vec::new();
    let mut finish_reason: Option<String> = None;
    while let Some(item) = stream.next().await {
        let chunk = item.map_err(ApiError::Internal)?;
        if chunk.prompt_tokens.is_some() {
            prompt_tokens = chunk.prompt_tokens.map(u64::from);
        }
        content.push_str(&chunk.content);
        completion_tokens += u64::from(chunk.tokens_emitted);
        if let Some(reason) = chunk.finish_reason {
            finish_reason = Some(reason);
        }
        if let Some(calls) = chunk.tool_calls {
            tool_deltas.extend(calls);
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    let tool_calls = merge_tool_calls(tool_deltas);
    // A turn that produced calls is a tool-call turn even if the engine's
    // own reason said "stop": the agent keys its next step off this field.
    let finish_reason = if tool_calls.is_empty() {
        finish_reason.unwrap_or_else(|| "stop".into())
    } else {
        "tool_calls".into()
    };

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
        &state.telemetry_cache,
        completion_tokens,
        elapsed,
    );

    let prompt_tokens = prompt_tokens.unwrap_or_else(|| {
        generation
            .messages
            .iter()
            .map(|m| approx_tokens(&m.content))
            .sum::<i64>() as u64
    });

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
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
                ..Default::default()
            },
            finish_reason,
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
    let cache = state.telemetry_cache.clone();

    let stream = async_stream::stream! {
        let started = Instant::now();
        yield Ok(sse_event(chunk_value(&id, &model, json!({ "role": ROLE_ASSISTANT }), None)));

        let mut content = String::new();
        let mut completion_tokens: u64 = 0;
        let mut finish_reason: Option<String> = None;
        let mut saw_tool_calls = false;
        let mut tool_indices: HashSet<u32> = HashSet::new();
        let mut backend = state.backend.generate(generation.clone());
        while let Some(item) = backend.next().await {
            match item {
                Ok(chunk) => {
                    completion_tokens += u64::from(chunk.tokens_emitted);
                    content.push_str(&chunk.content);
                    publish_metrics(&telemetry, &registry, &cache, completion_tokens, started.elapsed().as_secs_f64());
                    if !chunk.content.is_empty() {
                        yield Ok(sse_event(chunk_value(&id, &model, json!({ "content": chunk.content }), None)));
                    }
                    if let Some(mut calls) = chunk.tool_calls {
                        if !calls.is_empty() {
                            saw_tool_calls = true;
                            for call in &mut calls {
                                // Fill the id/type the engine may omit on the
                                // first fragment: OpenAI clients match the tool
                                // result to the call by id.
                                if tool_indices.insert(call.index) {
                                    if call.id.as_deref().unwrap_or("").is_empty() {
                                        call.id = Some(format!("call_{}", Uuid::new_v4()));
                                    }
                                    if call.kind.is_none() {
                                        call.kind = Some("function".into());
                                    }
                                }
                            }
                            yield Ok(sse_event(chunk_value(&id, &model, json!({ "tool_calls": calls }), None)));
                        }
                    }
                    if let Some(reason) = chunk.finish_reason {
                        finish_reason = Some(reason);
                    }
                }
                Err(error) => {
                    yield Ok(Event::default().data(json!({ "error": { "message": error } }).to_string()));
                    break;
                }
            }
        }

        let reason = finish_reason.unwrap_or_else(|| {
            if saw_tool_calls {
                "tool_calls".to_string()
            } else {
                "stop".to_string()
            }
        });
        yield Ok(sse_event(chunk_value(&id, &model, json!({}), Some(&reason))));
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

/// Assembles streamed fragments into complete calls, ordered by `index`.
fn merge_tool_calls(deltas: Vec<ToolCallDelta>) -> Vec<ToolCall> {
    let mut builders: BTreeMap<u32, ToolCall> = BTreeMap::new();
    for delta in deltas {
        let call = builders.entry(delta.index).or_insert_with(|| ToolCall {
            id: String::new(),
            kind: "function".into(),
            function: FunctionCall {
                name: String::new(),
                arguments: String::new(),
            },
        });
        if let Some(id) = delta.id {
            if !id.is_empty() {
                call.id = id;
            }
        }
        if let Some(kind) = delta.kind {
            if !kind.is_empty() {
                call.kind = kind;
            }
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name {
                if !name.is_empty() {
                    call.function.name = name;
                }
            }
            if let Some(arguments) = function.arguments {
                call.function.arguments.push_str(&arguments);
            }
        }
    }
    builders
        .into_values()
        .map(|mut call| {
            if call.id.is_empty() {
                call.id = format!("call_{}", Uuid::new_v4());
            }
            call
        })
        .collect()
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
    cache: &TelemetryCache,
    current_token: u64,
    elapsed_secs: f64,
) {
    let tokens_per_second = if elapsed_secs > 0.0 {
        current_token as f64 / elapsed_secs
    } else {
        0.0
    };
    // Read the cache, never dial: this runs per streaming token, and the poll
    // loop is the only thing that talks to the workers (spec 2026-10-22,
    // D-E1). A worker that has never answered has no snapshot, which is
    // "unknown" — not a zero.
    let worker_nodes = registry
        .list_nodes()
        .iter()
        .map(|node| {
            let health = cache.worker(&node.node_id);
            WorkerTelemetry {
                id: node.node_id.clone(),
                temp_celsius: health.as_ref().and_then(|h| h.temp_celsius),
                battery_pct: health.as_ref().and_then(|h| h.battery_pct),
            }
        })
        .collect();
    telemetry.publish(TelemetryEvent::generation_metrics(GenerationMetrics {
        tokens_per_second,
        current_token,
        master_ram_utilization_pct: cache.master_ram().and_then(|(value, _)| value),
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
    use crate::api::backend::FunctionCallDelta;
    use crate::discovery::mdns::DiscoveredNode;
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
            std::env::temp_dir().join("ai-upcycler-chat-test"),
        )
    }

    fn request(stream: bool) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "llama-8b".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hello there".into(),
                ..Default::default()
            }],
            temperature: 0.7,
            max_tokens: 64,
            stream,
            session_id: Some("s1".into()),
            tools: None,
            tool_choice: None,
            stop: None,
            top_p: None,
            seed: None,
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
    fn merge_tool_calls_assembles_fragments_by_index() {
        let calls = merge_tool_calls(vec![
            ToolCallDelta {
                index: 1,
                id: Some("call_2".into()),
                kind: Some("function".into()),
                function: Some(FunctionCallDelta {
                    name: Some("write".into()),
                    arguments: Some("{\"path\":".into()),
                }),
            },
            ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                kind: Some("function".into()),
                function: Some(FunctionCallDelta {
                    name: Some("read".into()),
                    arguments: Some("{\"path\":".into()),
                }),
            },
            ToolCallDelta {
                index: 0,
                id: None,
                kind: None,
                function: Some(FunctionCallDelta {
                    name: None,
                    arguments: Some("\"a.txt\"}".into()),
                }),
            },
        ]);

        assert_eq!(calls.len(), 2, "ordered by index");
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "read");
        assert_eq!(calls[0].function.arguments, "{\"path\":\"a.txt\"}");
        assert_eq!(calls[1].id, "call_2");
        assert_eq!(calls[1].function.name, "write");
    }

    /// The generation metrics carry the worker's **measured** health, and the
    /// master's real RAM utilization (spec 2026-10-22, D-E5/D-E6).
    #[test]
    fn generation_metrics_carry_the_cached_worker_health() {
        let state = seeded_state();
        state.registry.register_or_update(DiscoveredNode {
            node_id: "w1".into(),
            device_name: "Pixel 8".into(),
            ip_address: "127.0.0.1".into(),
            port: 50052,
            total_ram_bytes: 8 * 1024 * 1024 * 1024,
            free_ram_bytes: 4 * 1024 * 1024 * 1024,
            connection_type: "USB_ADB".into(),
            latency_ms: 2.0,
            last_seen_epoch_ms: 0,
        });
        state
            .telemetry_cache
            .record_worker(crate::telemetry::WorkerHealth {
                id: "w1".into(),
                temp_celsius: Some(38.5),
                battery_pct: Some(92.0),
                free_ram_bytes: 1024,
                observed_at_epoch_ms: 1,
                supports_compute: true,
            });
        state.telemetry_cache.record_master_ram(Some(42.0));

        let mut events = state.telemetry.subscribe();
        publish_metrics(
            &state.telemetry,
            &state.registry,
            &state.telemetry_cache,
            10,
            1.0,
        );

        let event = events.try_recv().expect("metrics event");
        match event {
            TelemetryEvent::GenerationMetrics { data, .. } => {
                assert_eq!(data.worker_nodes.len(), 1);
                assert_eq!(data.worker_nodes[0].temp_celsius, Some(38.5));
                assert_eq!(data.worker_nodes[0].battery_pct, Some(92.0));
                assert_eq!(data.master_ram_utilization_pct, Some(42.0));
            }
            other => panic!("expected generation metrics, got {other:?}"),
        }
    }

    /// A worker that has never answered is **unknown**, not zero (D-E3).
    #[test]
    fn generation_metrics_report_unknown_rather_than_zero() {
        let state = seeded_state();
        state.registry.register_or_update(DiscoveredNode {
            node_id: "w1".into(),
            device_name: "Pixel 8".into(),
            ip_address: "127.0.0.1".into(),
            port: 50052,
            total_ram_bytes: 8 * 1024 * 1024 * 1024,
            free_ram_bytes: 4 * 1024 * 1024 * 1024,
            connection_type: "USB_ADB".into(),
            latency_ms: 2.0,
            last_seen_epoch_ms: 0,
        });

        let mut events = state.telemetry.subscribe();
        publish_metrics(
            &state.telemetry,
            &state.registry,
            &state.telemetry_cache,
            10,
            1.0,
        );

        let event = events.try_recv().expect("metrics event");
        match event {
            TelemetryEvent::GenerationMetrics { data, .. } => {
                assert_eq!(data.worker_nodes[0].temp_celsius, None);
                assert_eq!(data.worker_nodes[0].battery_pct, None);
                assert_eq!(data.master_ram_utilization_pct, None);
            }
            other => panic!("expected generation metrics, got {other:?}"),
        }
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
            session_id: session_id.clone(),
            ..Default::default()
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
