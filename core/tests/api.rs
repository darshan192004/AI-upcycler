//! End-to-end tests exercising the HTTP router in-process (no sockets).

use ai_upcycler_core::api::backend::{
    GenerationBackend, GenerationChunk, GenerationRequest, StubBackend,
};
use ai_upcycler_core::api::downloads::StubFetcher;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::{DiscoveredNode, NodeRegistry};
use ai_upcycler_core::store::{NewModel, Store};
use ai_upcycler_core::telemetry::{DownloadStatus, TelemetryBus, TelemetryEvent};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use futures::stream::BoxStream;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

mod common;

use ai_upcycler_core::rpc::engine::LoadedPipeline;
use ai_upcycler_core::security::{EnrolledWorkerSource, ServerIdentity};
use common::{assignment, events, sample_payload, serve_mock_identity, MockWorker};

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
    .with_fetcher(Arc::new(StubFetcher::new()))
    .with_models_dir(
        std::env::temp_dir().join(format!("ai-upcycler-test-{}", uuid::Uuid::new_v4())),
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

/// A backend whose stream refuses mid-generation: the documented failure
/// (post-emission evictions/refusals fail honestly, plan PR-2) must surface
/// as an SSE `error` event, not as a silently-truncated answer.
struct RefusingBackend;

impl GenerationBackend for RefusingBackend {
    fn generate(
        &self,
        _request: GenerationRequest,
    ) -> BoxStream<'static, Result<GenerationChunk, String>> {
        Box::pin(async_stream::stream! {
            yield Ok(GenerationChunk {
                content: "partial".into(),
                tokens_emitted: 1,
                prompt_tokens: Some(3),
            });
            yield Err("session evicted mid-decode".into());
        })
    }
}

#[tokio::test]
async fn stream_failures_surface_as_sse_error_events() {
    let store = Arc::new(Store::open_in_memory().expect("store"));
    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 32 * 1024 * 1024,
            local_file_path: "/models/llama.gguf".into(),
        })
        .unwrap();
    let state = AppState::new(
        "cluster-1".into(),
        Arc::new(NodeRegistry::new()),
        store,
        Arc::new(RefusingBackend),
        TelemetryBus::new(64),
        HostInfo {
            node_id: "master".into(),
            device_name: "Master".into(),
            total_ram_mb: 2048,
            cpu_cores: 4,
        },
    );

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

    let response = router(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        text.contains("\"error\""),
        "the refusal must surface as an SSE error event: {text}"
    );
    assert!(
        text.contains("session evicted mid-decode"),
        "the honest refusal message must reach the client: {text}"
    );
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

#[tokio::test]
async fn rejects_non_gguf_download_requests() {
    let (status, body) = send(
        router(state()),
        "POST",
        "/api/models/download",
        Some(json!({ "repo_id": "owner/repo", "file_name": "model.safetensors" })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains(".gguf"));
}

#[tokio::test]
async fn rejects_download_path_traversal() {
    let (status, _) = send(
        router(state()),
        "POST",
        "/api/models/download",
        Some(json!({ "repo_id": "owner/repo", "file_name": "../evil.gguf" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        router(state()),
        "POST",
        "/api/models/download",
        Some(json!({ "repo_id": "owner/../repo", "file_name": "model.gguf" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pairing_session_returns_pin_qr_and_fingerprint() {
    let (status, body) = send(router(state()), "POST", "/api/pairing/session", None).await;

    assert_eq!(status, StatusCode::OK);
    let pin = body["pin"].as_str().unwrap();
    assert_eq!(pin.len(), 6);
    assert!(pin.chars().all(|c| c.is_ascii_digit()));
    let qr = body["qr_payload"].as_str().unwrap();
    assert!(qr.starts_with("aiupcycler://pair?"));
    assert!(qr.contains(&format!("token={pin}")));
    let fingerprint = body["fingerprint"].as_str().unwrap();
    assert_eq!(fingerprint.len(), 64);
    assert!(qr.contains(&format!("fp={fingerprint}")));
    assert!(body["expires_in_secs"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn enrolling_with_pin_issues_ca_signed_credentials() {
    let state = state();
    let app = router(state.clone());
    let (status, session) = send(app.clone(), "POST", "/api/pairing/session", None).await;
    assert_eq!(status, StatusCode::OK);
    let pin = session["pin"].as_str().unwrap().to_string();

    let (status, body) = send(
        app,
        "POST",
        "/api/pairing/enroll",
        Some(json!({
            "pin": pin,
            "device_name": "Pixel 8",
            "node_id": "phone-a",
            "addresses": ["192.168.1.50"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["ca_pem"]
        .as_str()
        .unwrap()
        .contains("BEGIN CERTIFICATE"));
    assert!(body["client_cert_pem"]
        .as_str()
        .unwrap()
        .contains("BEGIN CERTIFICATE"));
    assert!(body["client_key_pem"]
        .as_str()
        .unwrap()
        .contains("PRIVATE KEY"));
    // The worker's server identity: what the split driver dials it behind.
    assert!(body["server_cert_pem"]
        .as_str()
        .unwrap()
        .contains("BEGIN CERTIFICATE"));
    assert!(body["server_key_pem"]
        .as_str()
        .unwrap()
        .contains("PRIVATE KEY"));
    assert_ne!(
        body["server_cert_pem"].as_str().unwrap(),
        body["client_cert_pem"].as_str().unwrap(),
        "role separation: the two directions must not share a certificate"
    );
    assert_eq!(
        body["fingerprint"].as_str().unwrap(),
        session["fingerprint"].as_str().unwrap()
    );

    // The enrollment is recorded against the declared node id, carrying the
    // declared addresses — that is exactly what the driver's lookup is keyed by.
    let enrolled = state
        .security
        .enrollments
        .get("phone-a")
        .expect("enrollment recorded");
    assert_eq!(enrolled.device_name, "Pixel 8");
    assert_eq!(enrolled.addresses, vec!["192.168.1.50".to_string()]);
    assert!(!state.security.enrollments.is_enrolled("phone-b"));
}

#[tokio::test]
async fn enrolling_rejects_an_invalid_node_id_or_addresses() {
    let state = state();
    let app = router(state.clone());
    let (status, session) = send(app.clone(), "POST", "/api/pairing/session", None).await;
    assert_eq!(status, StatusCode::OK);
    let pin = session["pin"].as_str().unwrap().to_string();

    let enroll =
        |body: serde_json::Value| send(app.clone(), "POST", "/api/pairing/enroll", Some(body));

    // Blank node_id: without it the master cannot know which worker this is.
    let (status, _) = enroll(json!({
        "pin": pin, "device_name": "Pixel 8", "node_id": "  "
    }))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // An address that can be neither an IP literal nor a DNS name.
    let (status, _) = enroll(json!({
        "pin": pin, "device_name": "Pixel 8", "node_id": "phone-a",
        "addresses": ["not/valid"],
    }))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // An explicit empty list is rejected rather than silently defaulted.
    let (status, _) = enroll(json!({
        "pin": pin, "device_name": "Pixel 8", "node_id": "phone-a",
        "addresses": [],
    }))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // More addresses than the cap allows.
    let many: Vec<String> = (0..9).map(|i| format!("10.0.0.{i}")).collect();
    let (status, _) = enroll(json!({
        "pin": pin, "device_name": "Pixel 8", "node_id": "phone-a",
        "addresses": many,
    }))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A rejected request must not have spent the single-use PIN, and an omitted
    // address list falls back to the ADB-tunnel loopback pair.
    let (status, _) = enroll(json!({
        "pin": pin, "device_name": "Pixel 8", "node_id": "phone-a"
    }))
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a rejected request must not burn the PIN"
    );
    assert_eq!(
        state
            .security
            .enrollments
            .get("phone-a")
            .expect("enrolled")
            .addresses,
        vec!["127.0.0.1".to_string(), "localhost".to_string()]
    );
}

#[tokio::test]
async fn enrolling_rejects_wrong_reused_and_invalid_pins() {
    let app = router(state());
    let (status, session) = send(app.clone(), "POST", "/api/pairing/session", None).await;
    assert_eq!(status, StatusCode::OK);
    let pin = session["pin"].as_str().unwrap();

    // Wrong PIN → 403.
    let wrong = if pin == "111111" { "222222" } else { "111111" };
    let (status, _) = send(
        app.clone(),
        "POST",
        "/api/pairing/enroll",
        Some(json!({ "pin": wrong, "device_name": "Pixel 8", "node_id": "phone-a" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Valid PIN → 200, then reuse → 409.
    let (status, _) = send(
        app.clone(),
        "POST",
        "/api/pairing/enroll",
        Some(json!({ "pin": pin, "device_name": "Pixel 8", "node_id": "phone-a" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send(
        app.clone(),
        "POST",
        "/api/pairing/enroll",
        Some(json!({ "pin": pin, "device_name": "Pixel 8", "node_id": "phone-a" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Empty device name → 400 (fresh session so PIN state is irrelevant).
    let (status, session) = send(app.clone(), "POST", "/api/pairing/session", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        app,
        "POST",
        "/api/pairing/enroll",
        Some(json!({ "pin": session["pin"], "device_name": "  " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn download_conflicts_while_in_flight() {
    let state = state();
    state
        .active_downloads
        .lock()
        .await
        .insert("busy-model.gguf".to_string());

    let (status, body) = send(
        router(state),
        "POST",
        "/api/models/download",
        Some(json!({ "repo_id": "owner/repo", "file_name": "busy-model.gguf" })),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("already in progress"));
}

#[tokio::test]
async fn accepts_download_and_registers_model_on_completion() {
    let state = state();
    let mut receiver = state.telemetry.subscribe();
    let file_name = "tiny-model-q4_k_m.gguf";

    let (status, body) = send(
        router(state.clone()),
        "POST",
        "/api/models/download",
        Some(json!({ "repo_id": "owner/stub", "file_name": file_name })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["status"], "accepted");
    assert_eq!(body["file_name"], file_name);

    // Progress flows through the telemetry bus: running → complete.
    let complete = tokio::time::timeout(Duration::from_secs(5), async {
        let mut saw_running = false;
        loop {
            let event = receiver.recv().await.expect("telemetry bus open");
            if let TelemetryEvent::ModelDownloadProgress { data, .. } = event {
                match data.status {
                    DownloadStatus::Running => saw_running = true,
                    DownloadStatus::Complete => {
                        assert!(saw_running, "complete must follow a running event");
                        break data;
                    }
                    DownloadStatus::Error => panic!("download reported an error"),
                }
            }
        }
    })
    .await
    .expect("download completes");

    assert_eq!(complete.file_name, file_name);
    assert_eq!(complete.model_id, "tiny-model-q4_k_m");
    assert!(complete.total_bytes > 0);
    assert_eq!(complete.received_bytes, complete.total_bytes);

    // The downloaded file is registered in the store for /v1/models.
    let (status, models) = send(router(state.clone()), "GET", "/v1/models", None).await;
    assert_eq!(status, StatusCode::OK);
    let entry = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "tiny-model-q4_k_m")
        .expect("downloaded model listed");
    assert_eq!(entry["meta"]["quantization"], "Q4_K_M");
    assert_eq!(entry["meta"]["layers"], 16);

    // The GGUF payload actually landed on disk.
    let dest = state.models_dir.join(file_name);
    assert!(dest.is_file(), "downloaded file exists");
    let _ = std::fs::remove_file(dest);
}

/// Manual harness: serves the full API on 127.0.0.1:18081 until the process is
/// killed. Run with `cargo test --test api pairing_manual_server -- --ignored`
/// to exercise the pairing endpoints from a browser (Playwright).
#[tokio::test]
#[ignore = "manual harness: serves the API on 127.0.0.1:18081 until killed"]
async fn pairing_manual_server() {
    let addr: std::net::SocketAddr = "127.0.0.1:18081".parse().unwrap();
    ai_upcycler_core::api::serve(addr, state())
        .await
        .expect("serve");
}

/// The full provisioning loop: a worker pairs over the real HTTP API, and the
/// server certificate it is handed is the one the split driver dials it behind.
///
/// This is where the two halves meet — the endpoint issues and records the
/// identity, `EnrolledWorkerSource` resolves it from the declared node id, and
/// the driver completes an mTLS lifecycle using the PEM strings from the
/// response body.
#[tokio::test]
async fn an_enrolled_worker_is_driven_over_the_certificate_pairing_issued() {
    let state = state();
    let app = router(state.clone());

    let (status, session) = send(app.clone(), "POST", "/api/pairing/session", None).await;
    assert_eq!(status, StatusCode::OK);
    let pin = session["pin"].as_str().unwrap().to_string();

    let (status, enrolled) = send(
        app,
        "POST",
        "/api/pairing/enroll",
        Some(json!({
            "pin": pin,
            "device_name": "Pixel 8",
            "node_id": "phone-a",
            "addresses": ["127.0.0.1"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The worker gets its *own* key pair. Handing out the master's server
    // identity would leak the key the master serves its own API with.
    assert_ne!(
        enrolled["server_cert_pem"].as_str().unwrap(),
        state.security.ca.server_cert_pem(),
        "the master must never hand its own server identity to a worker"
    );

    // The worker serves gRPC behind exactly the identity it was handed, and
    // trusts only the CA from that same response.
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let server = ServerIdentity {
        cert_pem: enrolled["server_cert_pem"].as_str().unwrap().to_string(),
        key_pem: enrolled["server_key_pem"].as_str().unwrap().to_string(),
    };
    let (port, handle) = serve_mock_identity(
        MockWorker::new("phone-a", 0xA1, 100, log.clone()),
        &server,
        enrolled["ca_pem"].as_str().unwrap(),
    )
    .await;

    let mut pipeline = LoadedPipeline::load_secure(
        "llama-3-8b",
        &[assignment("phone-a", port, 0, 3)],
        &EnrolledWorkerSource::new(state.security.clone()),
    )
    .await
    .expect("load over the paired certificate");
    assert_eq!(pipeline.stage_count(), 1);

    let result = pipeline.forward(sample_payload()).await.expect("forward");
    assert_eq!(result.output_bytes, vec![0x10, 0x20, 0xA1]);
    pipeline.unload().await.expect("unload pipeline");

    assert_eq!(
        events(&log),
        vec![
            "handshake:phone-a",
            "load:phone-a:0..3",
            "forward:phone-a",
            "unload:phone-a:llama-3-8b",
        ]
    );

    handle.abort();
}

/// A worker the master never paired is refused before the network is touched:
/// port 1 is never bound, so a silent downgrade to plaintext would surface as a
/// connection error instead of the enrollment error asserted here.
#[tokio::test]
async fn an_unpaired_worker_is_never_dialed() {
    let state = state();
    let error = match LoadedPipeline::load_secure(
        "llama-3-8b",
        &[assignment("phone-a", 1, 0, 3)],
        &EnrolledWorkerSource::new(state.security.clone()),
    )
    .await
    {
        Ok(_) => panic!("an unpaired worker must not be reachable"),
        Err(error) => error,
    };
    assert!(
        error.contains("no completed pairing enrollment"),
        "got: {error}"
    );
}

/// The declared addresses are what bind the certificate to a dial target: a
/// worker that enrolled for some other address cannot be reached at this one.
///
/// Nothing else catches this — the load fails at the SAN check even though the
/// transport is otherwise perfectly configured, which is the whole point of
/// letting workers declare their addresses.
#[tokio::test]
async fn a_certificate_that_does_not_cover_the_dialed_address_is_refused() {
    let state = state();
    let app = router(state.clone());
    let (_, session) = send(app.clone(), "POST", "/api/pairing/session", None).await;
    let pin = session["pin"].as_str().unwrap().to_string();

    let (status, enrolled) = send(
        app,
        "POST",
        "/api/pairing/enroll",
        Some(json!({
            "pin": pin, "device_name": "Pixel 8", "node_id": "phone-a",
            // Not the loopback address the driver is about to dial.
            "addresses": ["10.9.9.9"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let server = ServerIdentity {
        cert_pem: enrolled["server_cert_pem"].as_str().unwrap().to_string(),
        key_pem: enrolled["server_key_pem"].as_str().unwrap().to_string(),
    };
    let (port, handle) = serve_mock_identity(
        MockWorker::new("phone-a", 0xA1, 100, log.clone()),
        &server,
        enrolled["ca_pem"].as_str().unwrap(),
    )
    .await;

    let error = match LoadedPipeline::load_secure(
        "llama-3-8b",
        &[assignment("phone-a", port, 0, 3)],
        &EnrolledWorkerSource::new(state.security.clone()),
    )
    .await
    {
        Ok(_) => panic!("a certificate issued for another address must not be accepted"),
        Err(error) => error,
    };
    assert!(
        !error.contains("enrollment"),
        "the enrollment lookup must have resolved; the refusal is the SAN check. got: {error}"
    );
    assert!(
        events(&log).is_empty(),
        "no RPC may complete against a worker whose certificate does not cover the dialed address"
    );

    handle.abort();
}
