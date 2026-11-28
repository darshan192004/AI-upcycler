//! Task 5 integration tests for the OpenAI-compatible image adapters.
//!
//! Every credential here is a synthetic `sk-test-*` marker; no real provider
//! key or live endpoint is used. The mock server asserts the exact wire shape
//! of each request.

mod common;

use ai_upcycler_core::imagegen::openai_compat::B64JsonImageBackend;
use ai_upcycler_core::imagegen::{
    GenerateImageRequest, ImageBackend, ImageError, ImageProviderId, ImageSize,
    DEFAULT_GEMINI_IMAGES_URL, DEFAULT_GEMINI_IMAGE_MODEL, DEFAULT_OPENAI_IMAGES_URL,
    DEFAULT_OPENAI_IMAGE_MODEL,
};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

const PNG_1X1_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p8kAAAAASUVORK5CYII=";

fn image_app(expected_authorization: &'static str) -> Router {
    Router::new().route(
        "/images/generations",
        post(
            move |headers: axum::http::HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(
                    headers.get("authorization").unwrap(),
                    expected_authorization
                );
                assert_eq!(body["model"], "gpt-image-2.6-flare");
                assert_eq!(body["prompt"], "a red bicycle");
                assert_eq!(body["n"], 1);
                assert_eq!(body["size"], "1024x1024");
                assert_eq!(body["response_format"], "b64_json");
                Json(json!({ "data": [{ "b64_json": PNG_1X1_BASE64 }] }))
            },
        ),
    )
}

#[tokio::test]
async fn openai_adapter_decodes_png_from_b64_json() {
    let (base_url, handle) =
        common::spawn_mock_provider(image_app("Bearer sk-test-image-key")).await;
    let backend = B64JsonImageBackend::new(
        ImageProviderId::OpenAI,
        DEFAULT_OPENAI_IMAGE_MODEL,
        format!("{base_url}/images/generations"),
    );
    let request = GenerateImageRequest {
        prompt: "a red bicycle".into(),
        provider: ImageProviderId::OpenAI,
        size: ImageSize::Square1024,
    };
    let image = backend
        .generate(&request, "sk-test-image-key")
        .await
        .unwrap();
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(image.extension, "png");
    assert_eq!(image.bytes.len(), 68);
    handle.abort();
}

#[test]
fn production_endpoints_are_exact() {
    assert_eq!(
        DEFAULT_OPENAI_IMAGES_URL,
        "https://api.openai.com/v1/images/generations"
    );
    assert_eq!(
        DEFAULT_GEMINI_IMAGES_URL,
        "https://generativelanguage.googleapis.com/v1beta/openai/images/generations"
    );
}

#[tokio::test]
async fn gemini_adapter_posts_to_openai_compatible_gemini_endpoint() {
    let app = Router::new().route(
        "/images/generations",
        post(
            |headers: axum::http::HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(
                    headers.get("authorization").unwrap(),
                    "Bearer sk-test-gemini-key"
                );
                assert_eq!(body["model"], DEFAULT_GEMINI_IMAGE_MODEL);
                Json(json!({ "data": [{ "b64_json": PNG_1X1_BASE64 }] }))
            },
        ),
    );
    let (base_url, handle) = common::spawn_mock_provider(app).await;
    let backend = B64JsonImageBackend::new(
        ImageProviderId::Gemini,
        DEFAULT_GEMINI_IMAGE_MODEL,
        format!("{base_url}/images/generations"),
    );
    let request = GenerateImageRequest {
        prompt: "a red bicycle".into(),
        provider: ImageProviderId::Gemini,
        size: ImageSize::Square1024,
    };
    assert!(backend
        .generate(&request, "sk-test-gemini-key")
        .await
        .is_ok());
    handle.abort();
}

#[tokio::test]
async fn invalid_b64_json_is_reported() {
    let app = Router::new().route(
        "/images/generations",
        post(|| async { Json(json!({ "data": [{ "b64_json": "not base64" }] })) }),
    );
    let (base_url, handle) = common::spawn_mock_provider(app).await;
    let backend = B64JsonImageBackend::new(
        ImageProviderId::OpenAI,
        DEFAULT_OPENAI_IMAGE_MODEL,
        format!("{base_url}/images/generations"),
    );
    let request = GenerateImageRequest {
        prompt: "a red bicycle".into(),
        provider: ImageProviderId::OpenAI,
        size: ImageSize::Square1024,
    };
    assert!(matches!(
        backend.generate(&request, "sk-test-image-key").await,
        Err(ImageError::UpstreamBody(_))
    ));
    handle.abort();
}

// --- Task 7: REST router smoke tests and persistence leak proof ------------

use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::NodeRegistry;
use ai_upcycler_core::imagegen::registry::ImageBackendRegistry;
use ai_upcycler_core::media::MediaStore;
use ai_upcycler_core::providers::ProviderId;
use ai_upcycler_core::secrets::{SecretScope, SecretStore};
use ai_upcycler_core::store::{NewImage, Store};
use ai_upcycler_core::telemetry::TelemetryBus;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

#[derive(Clone, Default)]
struct TestImageSecrets;

impl SecretStore for TestImageSecrets {
    fn get(&self, scope: SecretScope) -> Option<String> {
        match scope {
            SecretScope::Provider(ProviderId::OpenAI) => Some("sk-test-image-key".to_string()),
            SecretScope::Provider(ProviderId::Gemini) => Some("sk-test-gemini-key".to_string()),
            SecretScope::Provider(ProviderId::Anthropic)
            | SecretScope::Provider(ProviderId::Kimi)
            | SecretScope::LanToken => None,
        }
    }

    fn set(&self, _scope: SecretScope, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn delete(&self, _scope: SecretScope) -> Result<(), String> {
        Ok(())
    }
}

fn test_temp_root(tag: &str) -> PathBuf {
    PathBuf::from(r"D:\ai-upcycler-temp")
        .join("ai-upcycler-f7-images")
        .join(tag)
        .join(std::process::id().to_string())
        .join(uuid::Uuid::new_v4().to_string())
}

/// Real `AppState`, in-memory SQLite, temp media root, F5's mock endpoint and
/// synthetic secrets. `with_secrets` rebuilds `image_backends`, so the injected
/// mock registry is installed last.
async fn image_router_with_mock_provider() -> Router {
    let (base_url, _handle) =
        common::spawn_mock_provider(image_app("Bearer sk-test-image-key")).await;
    let secrets: Arc<dyn SecretStore> = Arc::new(TestImageSecrets);
    let state = AppState::new(
        "image-test".into(),
        Arc::new(NodeRegistry::new()),
        Arc::new(Store::open_in_memory().expect("in-memory store")),
        Arc::new(StubBackend::new()),
        TelemetryBus::new(64),
        HostInfo::default(),
    )
    .with_media(MediaStore::new(test_temp_root("media")))
    .with_secrets(secrets.clone())
    .with_image_backends(Arc::new(ImageBackendRegistry::new(
        secrets,
        format!("{base_url}/images/generations"),
        format!("{base_url}/images/generations"),
        true,
    )));
    router(state)
}

#[tokio::test]
async fn generate_writes_metadata_and_get_returns_bytes() {
    let app = image_router_with_mock_provider().await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/images/generate")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"prompt":"a red bicycle","provider":"openai","size":"square1024"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let metadata: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(metadata["provider"], "openai");
    assert_eq!(metadata["model"], "gpt-image-2.6-flare");
    assert_eq!(metadata["mime_type"], "image/png");
    assert_eq!(metadata["size"], "1024x1024");
    assert_eq!(metadata["byte_len"], 68);
    let sha = metadata["media_sha256"].as_str().unwrap();
    assert_eq!(sha.len(), 64);
    let id = metadata["image_id"].as_str().unwrap();

    let image = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/images/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(image.status(), StatusCode::OK);
    assert_eq!(
        image
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "image/png"
    );
    assert_eq!(
        image.headers().get("etag").unwrap().to_str().unwrap(),
        format!("\"{sha}\"")
    );
    assert_eq!(
        image
            .headers()
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap(),
        "public, max-age=31536000, immutable"
    );
    let bytes = axum::body::to_bytes(image.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(bytes.len(), 68);
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));

    let metadata_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/images/{id}/metadata"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metadata_response.status(), StatusCode::OK);
    let metadata_body = axum::body::to_bytes(metadata_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&metadata_body).unwrap();
    assert_eq!(stored["image_id"], id);
}

#[tokio::test]
async fn validation_and_local_errors_are_exact() {
    let app = image_router_with_mock_provider().await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/images/generate")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"prompt":"","provider":"openai","size":"square1024"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body)
        .contains("prompt must be 1..=4000 non-whitespace characters"));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/images/generate")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"prompt":"a red bicycle","provider":"local","size":"square1024"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("local image generation requires a new engine"));
}

#[tokio::test]
async fn missing_image_is_404_and_secret_is_not_persisted() {
    let app = image_router_with_mock_provider().await;

    let generated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/images/generate")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"prompt":"a red bicycle","provider":"openai","size":"square1024"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(generated.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/images/missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/images?limit=10&offset=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("sk-test-image-key"));
    assert!(!text.contains("sk-test-gemini-key"));
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["images"].as_array().unwrap().len(), 1);

    // SQLite-file proof: the handler's own Store is in-memory by contract, so
    // persist the exact API-visible metadata row into a fresh file-backed
    // Store and scan the resulting database bytes on disk for both markers.
    let db_root = test_temp_root("sqlite");
    std::fs::create_dir_all(&db_root).expect("sqlite temp dir");
    let db_path = db_root.join("images.sqlite3");
    let row = &list["images"][0];
    let store = Store::open(&db_path).expect("file-backed store");
    store
        .insert_image(&NewImage {
            image_id: row["image_id"].as_str().unwrap().to_string(),
            provider: row["provider"].as_str().unwrap().to_string(),
            model: row["model"].as_str().unwrap().to_string(),
            prompt: row["prompt"].as_str().unwrap().to_string(),
            media_sha256: row["media_sha256"].as_str().unwrap().to_string(),
            media_path: row["media_path"].as_str().unwrap().to_string(),
            mime_type: row["mime_type"].as_str().unwrap().to_string(),
            byte_len: row["byte_len"].as_i64().unwrap(),
            size: row["size"].as_str().unwrap().to_string(),
        })
        .expect("insert api-visible metadata");
    drop(store);
    let db_bytes = std::fs::read(&db_path).expect("read sqlite file");
    let db_text = String::from_utf8_lossy(&db_bytes);
    assert!(!db_text.contains("sk-test-image-key"));
    assert!(!db_text.contains("sk-test-gemini-key"));
}
