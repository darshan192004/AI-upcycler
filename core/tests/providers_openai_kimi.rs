//! Task 4 integration tests for the OpenAI-compatible adapters.
//!
//! Every credential here is a synthetic `sk-test-*` marker. The mock server
//! records the outgoing request so the tests can prove what went over the wire
//! without touching a real provider.

mod common;

use ai_upcycler_core::api::backend::{
    ChatMessage, ContentPart, FunctionDef, GenerationChunk, GenerationRequest, MessageContent,
    Stop, ToolDef,
};
use ai_upcycler_core::media::{MediaRef, MediaStore};
use ai_upcycler_core::providers::{descriptor_for, CloudRoute, ProviderId, ProviderRegistry};
use ai_upcycler_core::secrets::{SecretScope, SecretStore};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use futures::StreamExt;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

const OPENAI_KEY: &str = "sk-test-openai";
const KIMI_KEY: &str = "sk-test-kimi";

/// Two content fragments plus a finish-only chunk and `[DONE]`. The server
/// writes this as one body, so a single byte frame carries several `data:`
/// lines — the parser must not lose the boundaries.
const OPENAI_SSE: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"from OpenAI\"},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n",
);

const OPENAI_UNTERMINATED_SSE: &str =
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"},\"finish_reason\":null}]}\n\n";

const KIMI_SSE: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"from Kimi\"},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n",
);

#[derive(Debug, Default, Clone)]
struct Captured {
    authorization: String,
    body: Value,
}

struct FakeSecrets {
    provider: ProviderId,
    value: String,
}

impl SecretStore for FakeSecrets {
    fn get(&self, scope: SecretScope) -> Option<String> {
        match scope {
            SecretScope::Provider(id) if id == self.provider => Some(self.value.clone()),
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

/// A no-proxy client is a test-only concern: production keeps reqwest's normal
/// proxy behaviour, and loopback mock servers must never be routed elsewhere.
fn registry(provider: ProviderId, key: &str, base_url: String) -> ProviderRegistry {
    let secrets = FakeSecrets {
        provider,
        value: key.to_string(),
    };
    ProviderRegistry::new(Arc::new(secrets))
        .with_base_url(provider, base_url)
        .with_http_client(
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("no-proxy test client"),
        )
}

/// D:-only synthetic media root, unique per process and instant.
fn media_root(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    PathBuf::from(r"D:\ai-upcycler-temp")
        .join(tag)
        .join(format!("{}-{nanos}", std::process::id()))
}

/// Installs a media store so the adapter under test is the media-aware path.
fn registry_with_media_at(
    provider: ProviderId,
    key: &str,
    base_url: String,
    root: PathBuf,
) -> ProviderRegistry {
    registry(provider, key, base_url).with_media_store(MediaStore::new(root))
}

fn registry_with_media(provider: ProviderId, key: &str, base_url: String) -> ProviderRegistry {
    registry_with_media_at(provider, key, base_url, media_root("openai-media"))
}

/// Synthetic PNG-shaped bytes. The media store validates magic, not decoding,
/// so this keeps fixtures small while still exercising the real storage path.
fn attachment_bytes(len: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(len, 0);
    bytes
}

/// Writes a fixture directly under the content-addressed layout. Used only for
/// an object larger than `MediaStore::put` would accept, to prove the provider
/// budget is enforced from the stored object rather than request metadata.
fn write_raw_object(root: &Path, media_hash: &str, bytes: &[u8]) {
    assert_eq!(media_hash.len(), 64);
    let dir = root.join(&media_hash[..2]);
    std::fs::create_dir_all(&dir).expect("fixture dir");
    std::fs::write(dir.join(&media_hash[2..]), bytes).expect("fixture object");
}

fn image_part_from(stored: &MediaRef) -> ContentPart {
    ContentPart::Image {
        media_hash: stored.media_hash.clone(),
        mime_type: stored.mime_type.clone(),
        file_name: stored.file_name.clone(),
        byte_count: stored.byte_count,
    }
}

/// The request deliberately carries `tools` so the tests can prove that a
/// cloud adapter never forwards tool definitions.
fn generation_request(model: &str) -> GenerationRequest {
    GenerationRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "Be brief.".into(),
                ..Default::default()
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Say hello.".into(),
                ..Default::default()
            },
        ],
        temperature: 0.25,
        max_tokens: 64,
        tools: Some(vec![ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "read".to_string(),
                description: None,
                parameters: None,
            },
        }]),
        stop: Some(Stop::One("END".to_string())),
        // 0.5 is exact in both f32 and f64, so the wire assertion is stable.
        top_p: Some(0.5),
        seed: Some(7),
        ..Default::default()
    }
}

fn sse_app(captured: Arc<Mutex<Option<Captured>>>, body: &'static str) -> Router {
    Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, Json(payload): Json<Value>| {
            let captured = captured.clone();
            async move {
                *captured.lock().expect("captured lock") = Some(Captured {
                    authorization: headers
                        .get(AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body: payload,
                });
                ([(CONTENT_TYPE, "text/event-stream")], body.to_string())
            }
        }),
    )
}

async fn collect(
    registry: &ProviderRegistry,
    route: &CloudRoute,
    model: &str,
) -> Vec<GenerationChunk> {
    registry
        .stream(route, generation_request(model))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|chunk| chunk.expect("adapter stream must not fail"))
        .collect()
}

#[tokio::test]
async fn openai_sends_bearer_key_bare_model_and_stream_flag() {
    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle): (String, JoinHandle<()>) =
        common::spawn_mock_provider(sse_app(captured.clone(), OPENAI_SSE)).await;
    let registry = registry(ProviderId::OpenAI, OPENAI_KEY, format!("{base_url}/v1"));
    let route = CloudRoute {
        provider: ProviderId::OpenAI,
        model: "openai:gpt-4o".to_string(),
    };

    let chunks = collect(&registry, &route, "openai:gpt-4o").await;
    handle.abort();

    let seen = captured
        .lock()
        .expect("captured lock")
        .clone()
        .expect("request");
    assert_eq!(seen.authorization, format!("Bearer {OPENAI_KEY}"));
    assert_eq!(seen.body["model"], json!("gpt-4o"));
    assert_eq!(seen.body["stream"], json!(true));
    assert_eq!(seen.body["temperature"], json!(0.25));
    assert_eq!(seen.body["max_tokens"], json!(64));
    assert_eq!(seen.body["stop"], json!("END"));
    assert_eq!(seen.body["top_p"], json!(0.5));
    assert_eq!(seen.body["seed"], json!(7));
    assert_eq!(seen.body["messages"].as_array().map(Vec::len), Some(2));
    assert!(seen.body.get("tools").is_none(), "must never forward tools");
    let raw = seen.body.to_string();
    assert!(
        !raw.contains("openai:gpt-4o"),
        "model must be bare on the wire"
    );

    let text: String = chunks.iter().map(|chunk| chunk.content.as_str()).collect();
    assert_eq!(text, "Hello from OpenAI");
    assert!(chunks
        .iter()
        .filter(|chunk| !chunk.content.is_empty())
        .all(|chunk| chunk.tokens_emitted == 1));
    let final_chunk = chunks.last().expect("at least one chunk");
    assert_eq!(final_chunk.finish_reason.as_deref(), Some("stop"));
    assert_eq!(final_chunk.source.as_deref(), Some("openai"));
}

#[tokio::test]
async fn openai_non_200_error_names_provider_and_status_without_body_or_key() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            (
                StatusCode::UNAUTHORIZED,
                format!("secret-body-{OPENAI_KEY}"),
            )
        }),
    );
    let (base_url, handle) = common::spawn_mock_provider(app).await;
    let registry = registry(ProviderId::OpenAI, OPENAI_KEY, format!("{base_url}/v1"));
    let route = CloudRoute {
        provider: ProviderId::OpenAI,
        model: "gpt-4o".to_string(),
    };

    let errors: Vec<String> = registry
        .stream(&route, generation_request("gpt-4o"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(Result::err)
        .collect();
    handle.abort();

    let message = errors.first().expect("one error chunk");
    assert!(message.contains("openai"), "{message}");
    assert!(message.contains("401"), "{message}");
    assert!(
        !message.contains(OPENAI_KEY),
        "key leaked into error: {message}"
    );
    assert!(
        !message.contains("secret-body"),
        "body leaked into error: {message}"
    );
    assert!(errors.len() == 1, "expected only the error chunk");
}

#[tokio::test]
async fn kimi_uses_the_same_openai_compatible_shape() {
    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle) = common::spawn_mock_provider(sse_app(captured.clone(), KIMI_SSE)).await;
    let registry = registry(ProviderId::Kimi, KIMI_KEY, format!("{base_url}/v1"));
    let route = CloudRoute {
        provider: ProviderId::Kimi,
        model: "moonshot-v1-8k".to_string(),
    };

    let chunks = collect(&registry, &route, "kimi:moonshot-v1-8k").await;
    handle.abort();

    let seen = captured
        .lock()
        .expect("captured lock")
        .clone()
        .expect("request");
    assert_eq!(seen.authorization, format!("Bearer {KIMI_KEY}"));
    assert_eq!(seen.body["model"], json!("moonshot-v1-8k"));
    assert_eq!(seen.body["stream"], json!(true));
    assert!(seen.body.get("tools").is_none(), "must never forward tools");

    let text: String = chunks.iter().map(|chunk| chunk.content.as_str()).collect();
    assert_eq!(text, "Hello from Kimi");
    let final_chunk = chunks.last().expect("at least one chunk");
    assert_eq!(final_chunk.finish_reason.as_deref(), Some("stop"));
    assert_eq!(final_chunk.source.as_deref(), Some("kimi"));

    assert_eq!(
        descriptor_for(ProviderId::Kimi).default_base_url,
        "https://api.moonshot.ai/v1"
    );
}

#[tokio::test]
async fn openai_media_adapter_preserves_optional_message_metadata() {
    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle) =
        common::spawn_mock_provider(sse_app(captured.clone(), OPENAI_SSE)).await;
    let registry = registry_with_media(ProviderId::OpenAI, OPENAI_KEY, format!("{base_url}/v1"));
    let route = CloudRoute {
        provider: ProviderId::OpenAI,
        model: "gpt-4o".to_string(),
    };
    let mut request = generation_request("gpt-4o");
    request.messages[1].name = Some("analyst".into());

    let _ = registry.stream(&route, request).collect::<Vec<_>>().await;
    handle.abort();

    let seen = captured
        .lock()
        .expect("captured lock")
        .clone()
        .expect("request");
    assert_eq!(seen.body["messages"][1]["name"], json!("analyst"));
    assert_eq!(seen.body["messages"][1]["content"], json!("Say hello."));
}

#[tokio::test]
async fn openai_rejects_attachment_budget_violations_before_any_request() {
    const TEN_MIB: usize = 10 * 1024 * 1024;
    let root = media_root("openai-attachment-budget");
    let store = MediaStore::new(root.clone());
    let small = store
        .put(&attachment_bytes(64), "image/png", "small.png")
        .await
        .expect("small stored object");
    let full = store
        .put(&attachment_bytes(TEN_MIB), "image/png", "full.png")
        .await
        .expect("full stored object");

    // `put` refuses objects over its own cap, so write this one fixture into
    // the content-addressed layout directly. The provider must read the real
    // size rather than trusting a caller-supplied count.
    let oversized_hash = "cd".repeat(32);
    write_raw_object(&root, &oversized_hash, &attachment_bytes(TEN_MIB + 1));

    let mut oversized_file = generation_request("gpt-4o");
    oversized_file.messages[1].content = MessageContent::Parts(vec![ContentPart::Image {
        media_hash: oversized_hash,
        mime_type: "image/png".into(),
        file_name: "oversized.png".into(),
        byte_count: (TEN_MIB + 1) as u64,
    }]);

    let mut too_many_images = generation_request("gpt-4o");
    too_many_images.messages[1].content =
        MessageContent::Parts((0..5).map(|_| image_part_from(&small)).collect());

    let mut too_much_turn_data = generation_request("gpt-4o");
    too_much_turn_data.messages[1].content =
        MessageContent::Parts((0..4).map(|_| image_part_from(&full)).collect());
    too_much_turn_data.messages.push(ChatMessage {
        role: "user".into(),
        content: MessageContent::Parts(vec![image_part_from(&full)]),
        ..Default::default()
    });

    let registry = registry_with_media_at(
        ProviderId::OpenAI,
        OPENAI_KEY,
        "http://127.0.0.1:1/v1".to_string(),
        root.clone(),
    );
    let route = CloudRoute {
        provider: ProviderId::OpenAI,
        model: "gpt-4o".to_string(),
    };
    for (tag, request) in [
        ("oversized file", oversized_file),
        ("too many images", too_many_images),
        ("too much turn data", too_much_turn_data),
    ] {
        let outcomes: Vec<Result<GenerationChunk, String>> =
            registry.stream(&route, request).collect::<Vec<_>>().await;
        assert_eq!(outcomes.len(), 1, "{tag}: expected only an error");
        assert_eq!(
            outcomes[0].as_ref().expect_err("budget must fail"),
            "invalid_image_attachment",
            "{tag}"
        );
    }
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn openai_rejects_attachment_metadata_that_understates_stored_bytes() {
    let root = media_root("openai-attachment-mismatch");
    let store = MediaStore::new(root.clone());
    let stored = store
        .put(&attachment_bytes(64), "image/png", "synthetic.png")
        .await
        .expect("stored object");

    let mut request = generation_request("gpt-4o");
    request.messages[1].content = MessageContent::Parts(vec![ContentPart::Image {
        media_hash: stored.media_hash,
        mime_type: stored.mime_type,
        file_name: stored.file_name,
        byte_count: stored.byte_count.saturating_sub(1),
    }]);

    let registry = registry_with_media_at(
        ProviderId::OpenAI,
        OPENAI_KEY,
        "http://127.0.0.1:1/v1".to_string(),
        root.clone(),
    );
    let route = CloudRoute {
        provider: ProviderId::OpenAI,
        model: "gpt-4o".to_string(),
    };
    let outcomes: Vec<Result<GenerationChunk, String>> =
        registry.stream(&route, request).collect::<Vec<_>>().await;

    assert_eq!(outcomes.len(), 1, "only the metadata error is expected");
    assert_eq!(
        outcomes[0].as_ref().expect_err("mismatch must fail"),
        "invalid_image_attachment"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn openai_accepts_correctly_sized_stored_attachment() {
    let root = media_root("openai-attachment-valid");
    let store = MediaStore::new(root.clone());
    let bytes = attachment_bytes(64);
    let stored = store
        .put(&bytes, "image/png", "synthetic.png")
        .await
        .expect("stored object");

    let mut request = generation_request("gpt-4o");
    request.messages[1].content = MessageContent::Parts(vec![image_part_from(&stored)]);

    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle) =
        common::spawn_mock_provider(sse_app(captured.clone(), OPENAI_SSE)).await;
    let registry = registry_with_media_at(
        ProviderId::OpenAI,
        OPENAI_KEY,
        format!("{base_url}/v1"),
        root.clone(),
    );
    let route = CloudRoute {
        provider: ProviderId::OpenAI,
        model: "gpt-4o".to_string(),
    };
    let outcomes: Vec<Result<GenerationChunk, String>> =
        registry.stream(&route, request).collect::<Vec<_>>().await;
    handle.abort();

    assert!(
        outcomes.iter().all(|outcome| outcome.is_ok()),
        "a correctly sized attachment must remain valid"
    );
    let seen = captured
        .lock()
        .expect("captured lock")
        .clone()
        .expect("request");
    let expected = format!("data:image/png;base64,{}", STANDARD.encode(&bytes));
    assert_eq!(
        seen.body["messages"][1]["content"][0]["image_url"]["url"],
        json!(expected)
    );
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn openai_stream_without_completion_is_an_error() {
    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle) =
        common::spawn_mock_provider(sse_app(captured, OPENAI_UNTERMINATED_SSE)).await;
    let registry = registry(ProviderId::OpenAI, OPENAI_KEY, format!("{base_url}/v1"));
    let route = CloudRoute {
        provider: ProviderId::OpenAI,
        model: "gpt-4o".to_string(),
    };

    let outcomes: Vec<Result<GenerationChunk, String>> = registry
        .stream(&route, generation_request("gpt-4o"))
        .collect::<Vec<_>>()
        .await;
    handle.abort();

    assert_eq!(outcomes.len(), 2, "content chunk plus an EOF error");
    assert_eq!(outcomes[0].as_ref().expect("content").content, "Hello ");
    assert_eq!(
        outcomes[1].as_ref().expect_err("EOF must fail"),
        "openai stream ended before completion"
    );
}

#[tokio::test]
async fn kimi_stream_without_completion_is_an_error() {
    let (base_url, handle) =
        common::spawn_mock_provider(sse_app(Arc::new(Mutex::new(None)), OPENAI_UNTERMINATED_SSE))
            .await;
    let registry = registry(ProviderId::Kimi, KIMI_KEY, format!("{base_url}/v1"));
    let route = CloudRoute {
        provider: ProviderId::Kimi,
        model: "moonshot-v1-8k".to_string(),
    };

    let errors: Vec<String> = registry
        .stream(&route, generation_request("kimi:moonshot-v1-8k"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(Result::err)
        .collect();
    handle.abort();

    assert_eq!(
        errors,
        vec!["kimi stream ended before completion".to_string()]
    );
}

#[tokio::test]
async fn openai_oversized_sse_line_is_rejected_without_echoing_content() {
    let body = format!("data: {}\n\n", "x".repeat(1024 * 1024 + 1));
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let body = body.clone();
            async move { ([(CONTENT_TYPE, "text/event-stream")], body) }
        }),
    );
    let (base_url, handle) = common::spawn_mock_provider(app).await;
    let registry = registry(ProviderId::OpenAI, OPENAI_KEY, format!("{base_url}/v1"));
    let route = CloudRoute {
        provider: ProviderId::OpenAI,
        model: "gpt-4o".to_string(),
    };

    let errors: Vec<String> = registry
        .stream(&route, generation_request("gpt-4o"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(Result::err)
        .collect();
    handle.abort();

    assert_eq!(
        errors,
        vec!["openai returned an oversized response line".to_string()]
    );
}
