//! Cumulative router tests for the gated multimodal chat surface (Tasks 5, 6, 8, 9).
//!
//! Every temporary root stays on `D:\ai-upcycler-temp`; the process temp dir is
//! deliberately never used. All credentials below are synthetic markers.

mod common;

use ai_upcycler_core::api::backend::{
    GenerationBackend, GenerationChunk, GenerationRequest, StubBackend,
};
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::NodeRegistry;
use ai_upcycler_core::media::MediaStore;
use ai_upcycler_core::providers::{ProviderId, ProviderRegistry};
use ai_upcycler_core::secrets::{SecretScope, SecretStore};
use ai_upcycler_core::store::{NewModel, Store};
use ai_upcycler_core::telemetry::TelemetryBus;
use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use futures::stream::BoxStream;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tower::ServiceExt;

fn temp_root(tag: &str) -> PathBuf {
    PathBuf::from(r"D:\ai-upcycler-temp")
        .join("ai-upcycler-router")
        .join(tag)
        .join(format!("{}", std::process::id()))
        .join(uuid::Uuid::new_v4().to_string())
}

fn seed_store() -> Arc<Store> {
    let store = Arc::new(Store::open_in_memory().expect("in-memory store"));
    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 1024,
            local_file_path: "/models/llama.gguf".into(),
        })
        .expect("seed local model");
    store
}

fn test_state(root: PathBuf, enabled: bool) -> AppState {
    AppState::new(
        "multimodal-test".into(),
        Arc::new(NodeRegistry::new()),
        seed_store(),
        Arc::new(StubBackend::new()),
        TelemetryBus::new(64),
        HostInfo::default(),
    )
    .with_media_root(root)
    .with_multimodal(enabled)
}

fn build_test_router(enabled: bool, root: PathBuf) -> Router {
    router(test_state(root, enabled))
}

fn test_app(enabled: bool) -> Router {
    build_test_router(enabled, temp_root(&format!("app-{enabled}")))
}

/// A backend that reports a real engine so `RoutePolicy::Auto` selects the
/// local/cluster path. Every call is counted to prove refusals never reach it.
#[derive(Debug, Default)]
struct CountingBackend {
    calls: Arc<AtomicUsize>,
}

impl GenerationBackend for CountingBackend {
    fn generate(
        &self,
        _request: GenerationRequest,
    ) -> BoxStream<'static, Result<GenerationChunk, String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter(vec![Ok(GenerationChunk {
            content: "backend reached".into(),
            source: Some("local".into()),
            ..Default::default()
        })]))
    }

    fn is_real(&self) -> bool {
        true
    }
}

fn state_with_backend(
    root: PathBuf,
    enabled: bool,
    backend: Arc<dyn GenerationBackend>,
) -> (AppState, Arc<Store>) {
    let store = seed_store();
    let state = AppState::new(
        "multimodal-test".into(),
        Arc::new(NodeRegistry::new()),
        store.clone(),
        backend,
        TelemetryBus::new(64),
        HostInfo::default(),
    )
    .with_media_root(root)
    .with_multimodal(enabled);
    (state, store)
}

fn test_app_with_local_route() -> (Router, Arc<Store>, Arc<AtomicUsize>) {
    let backend = Arc::new(CountingBackend::default());
    let calls = backend.calls.clone();
    let (state, store) = state_with_backend(temp_root("local-route"), true, backend);
    (router(state), store, calls)
}

fn test_app_with_cluster_route() -> (Router, Arc<Store>, Arc<AtomicUsize>) {
    let backend = Arc::new(CountingBackend::default());
    let calls = backend.calls.clone();
    let (state, store) = state_with_backend(temp_root("cluster-route"), true, backend);
    (router(state), store, calls)
}

async fn json_body(res: Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn post_chat(app: &Router, body: Value) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn image_chat_body_for(model: &str, provider: Option<&str>) -> Value {
    let mut body = json!({
        "model":model,
        "messages":[{
            "role":"user",
            "content":[{
                "type":"image",
                "media_hash":"ab".repeat(32),
                "mime_type":"image/png",
                "file_name":"a.png",
                "byte_count":4
            }]
        }],
        "max_tokens":64
    });
    if let Some(provider) = provider {
        body["provider"] = json!(provider);
    }
    body
}

fn image_chat_body() -> Value {
    image_chat_body_for("llama-8b", None)
}

fn png_bytes() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\nhello".to_vec()
}

fn multipart_png(png: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        b"--x\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n",
    );
    body.extend_from_slice(png);
    body.extend_from_slice(b"\r\n--x--\r\n");
    body
}

const OPENAI_KEY: &str = "sk-test-openai";
const KIMI_KEY: &str = "sk-test-kimi";

const OPENAI_IMAGE_SSE: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"saw image\"},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n",
);

#[derive(Debug, Default, Clone)]
struct CapturedProviderRequest {
    authorization: String,
    body: Value,
}

struct TestSecrets {
    provider: ProviderId,
    key: &'static str,
}

impl SecretStore for TestSecrets {
    fn get(&self, scope: SecretScope) -> Option<String> {
        match scope {
            SecretScope::Provider(id) if id == self.provider => Some(self.key.to_string()),
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

/// Loopback provider mocks must never be routed through a system proxy.
fn registry_for(provider: ProviderId, key: &'static str, base_url: String) -> ProviderRegistry {
    ProviderRegistry::new(Arc::new(TestSecrets { provider, key }))
        .with_base_url(provider, base_url)
        .with_http_client(
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("no-proxy test client"),
        )
}

/// One OpenAI-compatible SSE response that also counts how many provider HTTP
/// calls arrived, so capability refusals can prove they never reached an adapter.
fn mock_openai_app(
    captured: Arc<Mutex<Option<CapturedProviderRequest>>>,
    hits: Arc<AtomicUsize>,
) -> Router {
    Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, Json(payload): Json<Value>| {
            let captured = captured.clone();
            let hits = hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                *captured.lock().expect("captured lock") = Some(CapturedProviderRequest {
                    authorization: headers
                        .get(AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body: payload,
                });
                ([(CONTENT_TYPE, "text/event-stream")], OPENAI_IMAGE_SSE)
            }
        }),
    )
}

/// Kimi speaks the same OpenAI-compatible wire shape; a distinct key makes a
/// mistaken route to OpenAI observable in the captured request.
fn mock_kimi_app(hits: Arc<AtomicUsize>) -> Router {
    Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, Json(payload): Json<Value>| {
            let hits = hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                let _ = (headers, payload);
                ([(CONTENT_TYPE, "text/event-stream")], OPENAI_IMAGE_SSE)
            }
        }),
    )
}

async fn spawn_openai_mock() -> (
    String,
    JoinHandle<()>,
    Arc<Mutex<Option<CapturedProviderRequest>>>,
    Arc<AtomicUsize>,
) {
    let captured = Arc::new(Mutex::new(None));
    let hits = Arc::new(AtomicUsize::new(0));
    let (base_url, handle) =
        common::spawn_mock_provider(mock_openai_app(captured.clone(), hits.clone())).await;
    (base_url, handle, captured, hits)
}

async fn spawn_kimi_mock() -> (String, JoinHandle<()>, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let (base_url, handle) = common::spawn_mock_provider(mock_kimi_app(hits.clone())).await;
    (base_url, handle, hits)
}

/// Builds a state whose media store already contains the bytes referenced by
/// the chat request, plus an injected registry bound to the mock provider.
fn state_with_registry_and_media(media: MediaStore, registry: ProviderRegistry) -> AppState {
    AppState::new(
        "multimodal-test".into(),
        Arc::new(NodeRegistry::new()),
        seed_store(),
        Arc::new(StubBackend::new()),
        TelemetryBus::new(64),
        HostInfo::default(),
    )
    .with_media(media)
    .with_multimodal(true)
    .with_providers(Arc::new(registry))
}

async fn seed_image(root: PathBuf) -> (MediaStore, String, u64) {
    let media = MediaStore::new(root);
    let reference = media
        .put(&png_bytes(), "image/png", "a.png")
        .await
        .expect("seed image bytes");
    (media, reference.media_hash, reference.byte_count)
}

fn set_image_ref(body: &mut Value, media_hash: &str, byte_count: u64) {
    body["messages"][0]["content"][0]["media_hash"] = json!(media_hash);
    body["messages"][0]["content"][0]["byte_count"] = json!(byte_count);
}

#[tokio::test]
async fn upload_rejects_when_gate_off() {
    let res = test_app(false)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/media")
                .body(Body::from("png"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn upload_stores_bytes_when_gate_on() {
    let png = png_bytes();
    let res = test_app(true)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/media")
                .header("content-type", "multipart/form-data; boundary=x")
                .body(Body::from(multipart_png(&png)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_body(res).await;
    assert_eq!(json["mime_type"], "image/png");
    assert_eq!(json["file_name"], "a.png");
    assert_eq!(json["media_hash"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn chat_with_image_is_rejected_when_gate_off() {
    let root = temp_root("gate-off");
    let state = test_state(root, false);
    let store = state.store.clone();
    let app = router(state);

    let res = post_chat(&app, image_chat_body()).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(res).await["error"], "multimodal_disabled");
    assert!(store.list_sessions().unwrap().is_empty());
}

#[tokio::test]
async fn local_route_with_image_returns_explicit_error() {
    let (app, store, calls) = test_app_with_local_route();

    let res = post_chat(&app, image_chat_body_for("llama-8b", Some("local"))).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(res).await["error"],
        "image_attachments_require_vision_provider"
    );
    assert!(store.list_sessions().unwrap().is_empty());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "backend must not be called"
    );
}

#[tokio::test]
async fn cluster_route_with_image_returns_explicit_error() {
    let (app, store, calls) = test_app_with_cluster_route();

    let res = post_chat(&app, image_chat_body()).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(res).await["error"],
        "image_attachments_require_vision_provider"
    );
    assert!(store.list_sessions().unwrap().is_empty());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "cluster must not be called"
    );
}

#[tokio::test]
async fn tool_turn_with_image_is_rejected_before_inference() {
    let (app, store, calls) = test_app_with_cluster_route();
    let body = json!({
        "model":"llama-8b",
        "messages":[{
            "role":"tool",
            "content":[{
                "type":"image",
                "media_hash":"ab".repeat(32),
                "mime_type":"image/png",
                "file_name":"a.png",
                "byte_count":4
            }]
        }],
        "max_tokens":64
    });

    let res = post_chat(&app, body).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(res).await["error"],
        "image_attachments_require_vision_provider"
    );
    assert!(store.list_sessions().unwrap().is_empty());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "backend must not be called"
    );
}

#[tokio::test]
async fn stream_with_image_emits_one_error_and_writes_no_session() {
    let root = temp_root("stream-refusal");
    let backend = Arc::new(CountingBackend::default());
    let calls = backend.calls.clone();
    let (state, store) = state_with_backend(root, true, backend);
    let app = router(state);
    let mut body = image_chat_body_for("llama-8b", Some("local"));
    body["stream"] = json!(true);

    let res = post_chat(&app, body).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(
        body.matches("image_attachments_require_vision_provider")
            .count(),
        1
    );
    assert!(store.list_sessions().unwrap().is_empty());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "backend must not be called"
    );
}

#[tokio::test]
async fn image_turn_posts_data_url_to_mock_openai() {
    let (base_url, handle, captured, hits) = spawn_openai_mock().await;
    let (media, media_hash, byte_count) = seed_image(temp_root("openai-image")).await;
    let registry = registry_for(ProviderId::OpenAI, OPENAI_KEY, format!("{base_url}/v1"));
    let app = router(state_with_registry_and_media(media, registry));
    let mut body = image_chat_body_for("openai:gpt-4o", Some("openai"));
    set_image_ref(&mut body, &media_hash, byte_count);

    let res = post_chat(&app, body).await;
    handle.abort();

    assert_eq!(res.status(), StatusCode::OK);
    let seen = captured
        .lock()
        .expect("captured lock")
        .clone()
        .expect("provider received a request");
    assert_eq!(seen.authorization, format!("Bearer {OPENAI_KEY}"));
    assert!(
        seen.body.to_string().contains("data:image/png;base64,"),
        "the adapter must resolve stored bytes to a data URL"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn image_with_non_vision_model_returns_400_before_http() {
    let (base_url, handle, hits) = spawn_kimi_mock().await;
    let (media, media_hash, byte_count) = seed_image(temp_root("kimi-non-vision")).await;
    let registry = registry_for(ProviderId::Kimi, KIMI_KEY, format!("{base_url}/v1"));
    let app = router(state_with_registry_and_media(media, registry));
    let mut body = image_chat_body_for("kimi:moonshot-v1-8k", Some("kimi"));
    set_image_ref(&mut body, &media_hash, byte_count);

    let res = post_chat(&app, body).await;
    handle.abort();

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(res).await["error"],
        "provider_lacks_vision_support"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no provider HTTP call");
}

#[tokio::test]
async fn image_with_no_vision_provider_returns_400() {
    let secrets = Arc::new(TestSecrets {
        provider: ProviderId::Kimi,
        key: KIMI_KEY,
    });
    let registry = ProviderRegistry::new(secrets);
    let media = MediaStore::new(temp_root("kimi-no-vision-model"));
    let app = router(state_with_registry_and_media(media, registry));

    let res = post_chat(
        &app,
        image_chat_body_for("kimi:unknown-model", Some("kimi")),
    )
    .await;

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(res).await["error"],
        "no_vision_capable_model_loaded"
    );
}

#[tokio::test]
async fn image_with_omitted_provider_follows_auto_route() {
    let (base_url, handle, captured, hits) = spawn_openai_mock().await;
    let (media, media_hash, byte_count) = seed_image(temp_root("openai-auto-route")).await;
    let registry = registry_for(ProviderId::OpenAI, OPENAI_KEY, format!("{base_url}/v1"));
    let app = router(state_with_registry_and_media(media, registry));
    let mut body = image_chat_body_for("openai:gpt-4o", None);
    set_image_ref(&mut body, &media_hash, byte_count);

    let res = post_chat(&app, body).await;
    handle.abort();

    assert_eq!(res.status(), StatusCode::OK);
    let seen = captured
        .lock()
        .expect("captured lock")
        .clone()
        .expect("provider received a request");
    assert_eq!(seen.authorization, format!("Bearer {OPENAI_KEY}"));
    assert!(seen.body.to_string().contains("data:image/png;base64,"));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
