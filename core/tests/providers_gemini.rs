//! Task 6 integration tests for the Gemini streaming adapter.
//!
//! The credential below is a synthetic marker. The mock server records the
//! outgoing request so the tests can prove the wire shape without contacting
//! a real provider.

mod common;

use ai_upcycler_core::api::backend::{
    ChatMessage, GenerationChunk, GenerationRequest, ToolDef, FunctionDef,
};
use ai_upcycler_core::providers::{CloudRoute, ProviderId, ProviderRegistry};
use ai_upcycler_core::secrets::{SecretScope, SecretStore};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const GEMINI_KEY: &str = "sk-test-gemini";

const GEMINI_SSE: &str = concat!(
    "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hello \"}]}}]}\n\n",
    "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"from Gemini\"}]}}]}\n\n",
    "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"!\"}]}}],\"usageMetadata\":{\"promptTokenCount\":11,\"candidatesTokenCount\":3}}\n\n",
);

#[derive(Debug, Default, Clone)]
struct Captured {
    api_key: String,
    path: String,
    query: String,
    body: Value,
}

struct FakeSecrets;

impl SecretStore for FakeSecrets {
    fn get(&self, scope: SecretScope) -> Option<String> {
        match scope {
            SecretScope::Provider(ProviderId::Gemini) => Some(GEMINI_KEY.to_string()),
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
        .with_base_url(ProviderId::Gemini, base_url)
        .with_http_client(
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("no-proxy test client"),
        )
}

fn generation_request() -> GenerationRequest {
    GenerationRequest {
        model: "gemini:gemini-2.5-flash".to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "Be brief.".to_string(),
                ..Default::default()
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Say hello.".to_string(),
                ..Default::default()
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "Earlier reply.".to_string(),
                ..Default::default()
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Now.".to_string(),
                ..Default::default()
            },
        ],
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
        "/v1beta/models/{model}:streamGenerateContent",
        post(move |axum::extract::Path(model): axum::extract::Path<String>,
                   uri: axum::http::Uri,
                   headers: axum::http::HeaderMap,
                   Json(payload): Json<Value>| {
            let captured = captured.clone();
            async move {
                let api_key = headers
                    .get("x-goog-api-key")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                *captured.lock().expect("captured lock") = Some(Captured {
                    api_key,
                    path: format!("/v1beta/models/{model}:streamGenerateContent"),
                    query: uri.query().unwrap_or_default().to_string(),
                    body: payload,
                });
                ([(axum::http::header::CONTENT_TYPE, "text/event-stream")], body.to_string())
            }
        }),
    )
}

#[tokio::test]
async fn gemini_sends_api_key_header_and_contents_then_reassembles_the_stream() {
    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle) =
        common::spawn_mock_provider(sse_app(captured.clone(), GEMINI_SSE)).await;
    let registry = registry(base_url);
    let route = CloudRoute {
        provider: ProviderId::Gemini,
        model: "gemini-2.5-flash".to_string(),
    };

    let chunks: Vec<GenerationChunk> = registry
        .stream(&route, generation_request())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|chunk| chunk.expect("adapter stream must not fail"))
        .collect();
    handle.abort();

    let seen = captured.lock().expect("captured lock").clone().expect("request");
    assert_eq!(seen.api_key, GEMINI_KEY);
    assert_eq!(seen.path, "/v1beta/models/gemini-2.5-flash:streamGenerateContent");
    assert_eq!(seen.query, "alt=sse", "key must not be in the query string");
    assert!(!seen.query.contains(GEMINI_KEY), "key leaked into URL");

    let contents = seen.body["contents"].as_array().expect("contents array");
    assert_eq!(contents.len(), 3, "system turns must be folded into a user turn");
    assert_eq!(contents[0]["role"], json!("user"));
    assert_eq!(
        contents[0]["parts"][0]["text"],
        json!("Be brief.\nSay hello."),
        "system text should prefix the first user part"
    );
    assert_eq!(contents[1]["role"], json!("model"));
    assert_eq!(contents[2]["role"], json!("user"));
    assert!(seen.body.get("tools").is_none(), "must never forward tools");

    let text: String = chunks.iter().map(|chunk| chunk.content.as_str()).collect();
    assert_eq!(text, "Hello from Gemini!");
    let final_chunk = chunks.last().expect("at least one chunk");
    assert_eq!(final_chunk.prompt_tokens, Some(11));
    assert_eq!(final_chunk.tokens_emitted, 3);
    assert_eq!(final_chunk.finish_reason.as_deref(), Some("stop"));
    assert_eq!(final_chunk.source.as_deref(), Some("gemini"));
    assert!(chunks
        .iter()
        .filter(|chunk| !chunk.content.is_empty())
        .all(|chunk| chunk.source.as_deref() == Some("gemini")));
}

#[tokio::test]
async fn gemini_non_200_error_names_provider_and_status_without_key_or_body() {
    let app = Router::new().route(
        "/v1beta/models/{model}:streamGenerateContent",
        post(|| async {
            (
                StatusCode::UNAUTHORIZED,
                format!("secret-body-{GEMINI_KEY}"),
            )
        }),
    );
    let (base_url, handle) = common::spawn_mock_provider(app).await;
    let registry = registry(base_url);
    let route = CloudRoute {
        provider: ProviderId::Gemini,
        model: "gemini-2.5-flash".to_string(),
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
    assert!(message.contains("gemini"), "{message}");
    assert!(message.contains("401"), "{message}");
    assert!(!message.contains(GEMINI_KEY), "key leaked: {message}");
    assert!(!message.contains("secret-body"), "body leaked: {message}");
}
