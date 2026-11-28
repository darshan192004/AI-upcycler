//! Task 5 integration tests for the Anthropic Messages adapter.
//!
//! The credential below is a synthetic marker. The mock server records the
//! outgoing request so the tests can prove the wire shape without contacting
//! a real provider.

mod common;

use ai_upcycler_core::api::backend::{
    ChatMessage, FunctionDef, GenerationChunk, GenerationRequest, Stop, ToolDef,
};
use ai_upcycler_core::providers::{CloudRoute, ProviderId, ProviderRegistry};
use ai_upcycler_core::secrets::{SecretScope, SecretStore};
use axum::http::header::{HeaderMap, AUTHORIZATION};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const ANTHROPIC_KEY: &str = "sk-test-anthropic";
const ANTHROPIC_VERSION: &str = "2023-06-01";

const ANTHROPIC_SSE: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":9}}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello \"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"from Anthropic\"}}\n\n",
    "event: message_delta\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n\n",
    "event: message_stop\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);

#[derive(Debug, Default, Clone)]
struct Captured {
    api_key: String,
    version: String,
    authorization: String,
    body: Value,
}

struct FakeSecrets;

impl SecretStore for FakeSecrets {
    fn get(&self, scope: SecretScope) -> Option<String> {
        match scope {
            SecretScope::Provider(ProviderId::Anthropic) => Some(ANTHROPIC_KEY.to_string()),
            _ => None,
        }
    }

    fn set(&self, _scope: SecretScope, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn delete(&self, _scope: SecretScope) -> Result<(), String> {
        Ok(())
    }
}

fn registry(base_url: String) -> ProviderRegistry {
    ProviderRegistry::new(Arc::new(FakeSecrets))
        .with_base_url(ProviderId::Anthropic, base_url)
        .with_http_client(
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("no-proxy test client"),
        )
}

fn generation_request() -> GenerationRequest {
    GenerationRequest {
        model: "anthropic:claude-3-5-sonnet-latest".to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "Be brief.".into(),
                ..Default::default()
            },
            ChatMessage {
                role: "system".to_string(),
                content: "Be kind.".into(),
                ..Default::default()
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Say hello.".into(),
                ..Default::default()
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "Earlier reply.".into(),
                ..Default::default()
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Now.".into(),
                ..Default::default()
            },
        ],
        temperature: 0.25,
        max_tokens: 64,
        stop: Some(Stop::One("END".to_string())),
        tools: Some(vec![ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "read".to_string(),
                description: None,
                parameters: None,
            },
        }]),
        ..Default::default()
    }
}

fn sse_app(captured: Arc<Mutex<Option<Captured>>>, body: &'static str) -> Router {
    Router::new().route(
        "/v1/messages",
        post(move |headers: HeaderMap, Json(payload): Json<Value>| {
            let captured = captured.clone();
            async move {
                let header = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string()
                };
                *captured.lock().expect("captured lock") = Some(Captured {
                    api_key: header("x-api-key"),
                    version: header("anthropic-version"),
                    authorization: header(AUTHORIZATION.as_str()),
                    body: payload,
                });
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    body.to_string(),
                )
            }
        }),
    )
}

#[tokio::test]
async fn anthropic_sends_headers_and_contract_payload_then_reassembles_the_stream() {
    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle) =
        common::spawn_mock_provider(sse_app(captured.clone(), ANTHROPIC_SSE)).await;
    let registry = registry(base_url);
    let route = CloudRoute {
        provider: ProviderId::Anthropic,
        model: "claude-3-5-sonnet-latest".to_string(),
    };

    let chunks: Vec<GenerationChunk> = registry
        .stream(&route, generation_request())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|chunk| chunk.expect("adapter stream must not fail"))
        .collect();
    handle.abort();

    let seen = captured
        .lock()
        .expect("captured lock")
        .clone()
        .expect("request");
    assert_eq!(seen.api_key, ANTHROPIC_KEY);
    assert_eq!(seen.version, ANTHROPIC_VERSION);
    assert!(
        seen.authorization.is_empty(),
        "must not send a Bearer header"
    );
    assert_eq!(seen.body["model"], json!("claude-3-5-sonnet-latest"));
    assert_eq!(seen.body["system"], json!("Be brief.\nBe kind."));
    assert_eq!(seen.body["stream"], json!(true));
    assert_eq!(seen.body["max_tokens"], json!(64));
    let messages = seen.body["messages"].as_array().expect("messages array");
    assert_eq!(
        messages.len(),
        3,
        "system messages must not be forwarded as turns"
    );
    assert_eq!(messages[0]["role"], json!("user"));
    assert_eq!(messages[1]["role"], json!("assistant"));
    assert_eq!(messages[2]["role"], json!("user"));
    assert!(seen.body.get("tools").is_none(), "must never forward tools");

    let text: String = chunks.iter().map(|chunk| chunk.content.as_str()).collect();
    assert_eq!(text, "Hello from Anthropic");
    assert_eq!(chunks[0].prompt_tokens, Some(9));
    let final_chunk = chunks.last().expect("at least one chunk");
    assert_eq!(final_chunk.finish_reason.as_deref(), Some("end_turn"));
    assert_eq!(final_chunk.source.as_deref(), Some("anthropic"));
    assert!(chunks
        .iter()
        .filter(|chunk| !chunk.content.is_empty())
        .all(|chunk| chunk.source.as_deref() == Some("anthropic")));
}

#[tokio::test]
async fn anthropic_non_200_error_names_provider_and_status_without_key_or_body() {
    let app = Router::new().route(
        "/v1/messages",
        post(|| async {
            (
                StatusCode::UNAUTHORIZED,
                format!("secret-body-{ANTHROPIC_KEY}"),
            )
        }),
    );
    let (base_url, handle) = common::spawn_mock_provider(app).await;
    let registry = registry(base_url);
    let route = CloudRoute {
        provider: ProviderId::Anthropic,
        model: "claude-3-5-sonnet-latest".to_string(),
    };

    let errors: Vec<String> = registry
        .stream(&route, generation_request())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(Result::err)
        .collect();
    handle.abort();

    assert_eq!(errors.len(), 1, "expected only the error chunk");
    let message = &errors[0];
    assert!(message.contains("anthropic"), "{message}");
    assert!(message.contains("401"), "{message}");
    assert!(!message.contains(ANTHROPIC_KEY), "key leaked: {message}");
    assert!(!message.contains("secret-body"), "body leaked: {message}");
}
