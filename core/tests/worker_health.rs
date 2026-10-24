//! Worker health polling end to end (spec 2026-10-22).
//!
//! The workers already measure themselves; the master never asked and
//! published `0.0` placeholders. These tests run an in-proc mock worker over
//! loopback plaintext and assert the whole loop: the snapshot the dashboard
//! shows came from the wire, an unknown metric is `null` rather than zero, a
//! worker that stops answering keeps its last real snapshot, and the
//! generation path never dials anybody for health.

use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::{DiscoveredNode, NodeRegistry};
use ai_upcycler_core::rpc::client::WorkerRpcClient;
use ai_upcycler_core::rpc::engine::{WorkerTransport, WorkerTransportSource};
use ai_upcycler_core::store::{NewModel, Store};
use ai_upcycler_core::telemetry::{TelemetryBus, TelemetryEvent};
use ai_upcycler_core::worker_health::{poll_once, HostMemorySource, TelemetryCache};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

mod common;

use common::{serve_mock, MockWorker};

/// Plaintext for every node: loopback mock workers are not enrolled, and
/// production never dials an unenrolled node over plaintext.
struct PlaintextTransports;

impl WorkerTransportSource for PlaintextTransports {
    fn transport_for(&self, _node_id: &str) -> Result<WorkerTransport, String> {
        Ok(WorkerTransport::Plaintext)
    }
}

/// A master whose RAM reading is fixed, so the test never depends on the
/// machine it runs on.
struct FixedHostMemory(f64);

impl HostMemorySource for FixedHostMemory {
    fn utilization_pct(&self) -> Option<f64> {
        Some(self.0)
    }
}

struct UnreadableHostMemory;

impl HostMemorySource for UnreadableHostMemory {
    fn utilization_pct(&self) -> Option<f64> {
        None
    }
}

fn node(node_id: &str, port: u16) -> DiscoveredNode {
    DiscoveredNode {
        node_id: node_id.into(),
        device_name: format!("device-{node_id}"),
        ip_address: "127.0.0.1".into(),
        port,
        total_ram_bytes: 8 * 1024 * 1024 * 1024,
        free_ram_bytes: 4 * 1024 * 1024 * 1024,
        connection_type: "USB_ADB".into(),
        latency_ms: 2.0,
        last_seen_epoch_ms: 0,
    }
}

/// The measured path: the snapshot equals the values the worker reported —
/// an equality assertion, so a hash-derived or zero-filled value fails.
#[tokio::test]
async fn the_poll_records_the_values_the_worker_reported() {
    let (port, _handle) = serve_mock(
        MockWorker::new("w1", 7, 10, Arc::new(Mutex::new(Vec::new()))).with_health(92.0, 38.5),
    )
    .await;
    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("w1", port));

    let telemetry = TelemetryBus::new(8);
    let mut events = telemetry.subscribe();
    let cache = Arc::new(TelemetryCache::new());

    poll_once(
        &registry,
        &PlaintextTransports,
        &telemetry,
        &cache,
        &FixedHostMemory(42.0),
    )
    .await;

    let health = cache.worker("w1").expect("snapshot for w1");
    assert_eq!(health.battery_pct, Some(92.0));
    assert_eq!(health.temp_celsius, Some(38.5));
    assert!(health.free_ram_bytes > 0);
    assert!(health.observed_at_epoch_ms > 0);
    assert!(health.supports_compute, "mock worker advertises compute");

    let event = events.try_recv().expect("worker_health event");
    match event {
        TelemetryEvent::WorkerHealth { data, .. } => {
            assert_eq!(data.nodes.len(), 1);
            assert_eq!(data.nodes[0].id, "w1");
            assert_eq!(data.nodes[0].battery_pct, Some(92.0));
            assert_eq!(data.master_ram_utilization_pct, Some(42.0));
        }
        other => panic!("expected a worker_health event, got {other:?}"),
    }
}

/// An unknown metric is `null`, never `0.0` (D-E3): the worker says "I cannot
/// measure this" with a negative sentinel.
#[tokio::test]
async fn an_unknown_metric_is_null_not_zero() {
    let (port, _handle) = serve_mock(
        MockWorker::new("w1", 7, 10, Arc::new(Mutex::new(Vec::new()))).with_health(-1.0, -1.0),
    )
    .await;
    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("w1", port));

    let telemetry = TelemetryBus::new(8);
    let cache = Arc::new(TelemetryCache::new());

    poll_once(
        &registry,
        &PlaintextTransports,
        &telemetry,
        &cache,
        &FixedHostMemory(42.0),
    )
    .await;

    let health = cache.worker("w1").expect("snapshot for w1");
    assert_eq!(health.battery_pct, None, "battery is unknown, not 0%");
    assert_eq!(health.temp_celsius, None, "temperature is unknown, not 0C");
}

/// A worker that stops answering keeps its last real snapshot (D-E2): the
/// value was really measured, and its timestamp says how old it is.
#[tokio::test]
async fn a_worker_that_stops_answering_keeps_its_last_snapshot() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let (port, handle) =
        serve_mock(MockWorker::new("w1", 7, 10, events.clone()).with_health(92.0, 38.5)).await;
    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("w1", port));

    let telemetry = TelemetryBus::new(8);
    let cache = Arc::new(TelemetryCache::new());
    let transports = PlaintextTransports;

    poll_once(
        &registry,
        &transports,
        &telemetry,
        &cache,
        &FixedHostMemory(42.0),
    )
    .await;
    let first = cache.worker("w1").expect("first snapshot");
    assert_eq!(first.battery_pct, Some(92.0));

    // The worker goes away; the port is closed.
    handle.abort();
    let _ = handle.await;

    poll_once(
        &registry,
        &transports,
        &telemetry,
        &cache,
        &FixedHostMemory(42.0),
    )
    .await;

    let kept = cache.worker("w1").expect("snapshot is kept, not dropped");
    assert_eq!(kept.battery_pct, Some(92.0));
    assert_eq!(kept.temp_celsius, Some(38.5));
}

/// A worker that leaves the registry is dropped from the cache (D-E8).
#[tokio::test]
async fn a_departed_worker_is_dropped() {
    let (port, _handle) = serve_mock(
        MockWorker::new("w1", 7, 10, Arc::new(Mutex::new(Vec::new()))).with_health(92.0, 38.5),
    )
    .await;
    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("w1", port));

    let telemetry = TelemetryBus::new(8);
    let cache = Arc::new(TelemetryCache::new());

    poll_once(
        &registry,
        &PlaintextTransports,
        &telemetry,
        &cache,
        &FixedHostMemory(42.0),
    )
    .await;
    assert!(cache.worker("w1").is_some());

    registry.remove("w1");
    poll_once(
        &registry,
        &PlaintextTransports,
        &telemetry,
        &cache,
        &FixedHostMemory(42.0),
    )
    .await;
    assert!(cache.worker("w1").is_none());
}

/// A master whose memory cannot be read reports "unknown", not `0%` (D-E6).
#[tokio::test]
async fn an_unreadable_master_memory_is_unknown() {
    let registry = Arc::new(NodeRegistry::new());
    let telemetry = TelemetryBus::new(8);
    let cache = Arc::new(TelemetryCache::new());

    poll_once(
        &registry,
        &PlaintextTransports,
        &telemetry,
        &cache,
        &UnreadableHostMemory,
    )
    .await;

    assert_eq!(cache.master_ram().expect("a reading exists").0, None);
}

/// The generation path never dials a worker for health (D-E1).
///
/// `publish_metrics` runs per streaming token, so a health RPC in it would be
/// one round trip per token. The mock worker records every RPC it receives,
/// so an empty log is the proof that the hot path only reads the cache.
#[tokio::test]
async fn the_generation_path_never_asks_a_worker_for_health() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let (port, _handle) =
        serve_mock(MockWorker::new("w1", 7, 10, events.clone()).with_health(92.0, 38.5)).await;

    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("w1", port));
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
        .expect("model");
    let state = AppState::new(
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
    );

    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "model": "llama-8b",
                "stream": true,
                "messages": [{ "role": "user", "content": "hello" }]
            }))
            .expect("body"),
        ))
        .expect("request");

    // The mock logs every call it receives: prove the log works by asking for
    // health once, then clear it. Without this the assertion below would also
    // hold if the mock simply stopped logging.
    {
        let mut client = WorkerRpcClient::connect(&format!("http://127.0.0.1:{port}"))
            .await
            .expect("connect");
        client.health().await.expect("health");
    }
    let logged = events.lock().expect("events lock").clone();
    assert!(
        logged.iter().any(|call| call.starts_with("health:")),
        "the mock must record health calls, otherwise the assertion below is vacuous"
    );
    events.lock().expect("events lock").clear();

    let response = router(state).oneshot(request).await.expect("router");
    assert_eq!(response.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");

    let calls = events.lock().expect("events lock").clone();
    assert!(
        !calls.iter().any(|call| call.starts_with("health:")),
        "the generation path must not dial for health, but the worker saw {calls:?}"
    );
}
