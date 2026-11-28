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
