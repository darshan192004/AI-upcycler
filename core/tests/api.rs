//! End-to-end tests exercising the HTTP router in-process (no sockets).

use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::{DiscoveredNode, NodeRegistry};
use ai_upcycler_core::store::{NewModel, Store};
use ai_upcycler_core::telemetry::TelemetryBus;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

fn state() -> AppState {
    let store = Arc::new(Store::open_in_memory().expect("store"));
    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 32 * 1024 * 1024 * 64,
            local_file_path: "/models/llama.gguf".into(),
        })
        .unwrap();

    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(DiscoveredNode {
        node_id: "worker-1".into(),
        device_name: "Pixel 8".into(),
        ip_address: "192.168.1.50".into(),
        port: 50052,
        total_ram_bytes: 8 * 1024 * 1024 * 1024,
        free_ram_bytes: 4 * 1024 * 1024 * 1024,
        connection_type: "USB_ADB".into(),
        latency_ms: 2.0,
        last_seen_epoch_ms: 0,
    });

    AppState::new(
        "cluster-1".into(),
        registry,
        store,
        Arc::new(StubBackend::new()),
        TelemetryBus::new(64),
        HostInfo {
            node_id: "master".into(),
            device_name: "Master".into(),
            total_ram_mb: 2048,
            cpu_cores: 4,
        },
    )
}

async fn send(
    app: Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            builder
                .body(Body::from(serde_json::to_vec(&value).unwrap()))
                .unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

#[tokio::test]
async fn lists_models() {
    let (status, body) = send(router(state()), "GET", "/v1/models", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");
    assert_eq!(body["data"][0]["id"], "llama-8b");
    assert_eq!(body["data"][0]["meta"]["quantization"], "Q4_K_M");
}

#[tokio::test]
async fn completes_chat_without_streaming() {
    let (status, body) = send(
        router(state()),
        "POST",
        "/v1/chat/completions",
        Some(json!({
            "model": "llama-8b",
            "messages": [{ "role": "user", "content": "hello cluster" }]
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains("[stub:llama-8b]"));
    assert!(body["usage"]["completion_tokens"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn streams_chat_as_sse() {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "model": "llama-8b",
                "stream": true,
                "messages": [{ "role": "user", "content": "stream this" }]
            }))
            .unwrap(),
        ))
        .unwrap();

    let response = router(state()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("chat.completion.chunk"));
    assert!(text.contains("data: [DONE]"));
}

#[tokio::test]
async fn rejects_unknown_model() {
    let (status, body) = send(
        router(state()),
        "POST",
        "/v1/chat/completions",
        Some(json!({
            "model": "ghost",
            "messages": [{ "role": "user", "content": "hi" }]
        })),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"]["message"].as_str().unwrap().contains("ghost"));
}

#[tokio::test]
async fn exposes_cluster_topology() {
    let (status, body) = send(router(state()), "GET", "/api/cluster/topology", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cluster_id"], "cluster-1");
    assert_eq!(body["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(body["nodes"][0]["role"], "master");
    assert_eq!(body["nodes"][1]["connection"], "usb_adb");
}

#[tokio::test]
async fn runs_benchmark() {
    let (status, body) = send(router(state()), "POST", "/api/cluster/benchmark", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "success");
    assert_eq!(body["nodes_tested"], 1);
    assert_eq!(body["rebalancing_triggered"], true);
}
