//! The topology reports the thermal state the layer plan was computed against,
//! and a hot worker rests while a cool one carries the load (spec
//! 2026-10-22, D-F2/D-F4).
//!
//! The placement arithmetic is covered by the planner's own unit tests; this
//! file proves the wiring: the temperature the poll measured reaches the
//! plan, and the plan's thermal state reaches the API.

use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::{DiscoveredNode, NodeRegistry};
use ai_upcycler_core::rpc::engine::{WorkerTransport, WorkerTransportSource};
use ai_upcycler_core::store::{NewModel, Store};
use ai_upcycler_core::telemetry::TelemetryBus;
use ai_upcycler_core::worker_health::{poll_once, HostMemorySource, TelemetryCache};
use axum::body::Body;
use axum::http::Request;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

mod common;

use common::{serve_mock, MockWorker};

struct PlaintextTransports;

impl WorkerTransportSource for PlaintextTransports {
    fn transport_for(&self, _node_id: &str) -> Result<WorkerTransport, String> {
        Ok(WorkerTransport::Plaintext)
    }
}

struct FixedHostMemory(f64);

impl HostMemorySource for FixedHostMemory {
    fn utilization_pct(&self) -> Option<f64> {
        Some(self.0)
    }
}

fn node(node_id: &str, port: u16, ram_bytes: u64) -> DiscoveredNode {
    DiscoveredNode {
        node_id: node_id.into(),
        device_name: format!("device-{node_id}"),
        ip_address: "127.0.0.1".into(),
        port,
        total_ram_bytes: ram_bytes,
        free_ram_bytes: ram_bytes,
        connection_type: "USB_ADB".into(),
        latency_ms: 2.0,
        last_seen_epoch_ms: 0,
    }
}

#[tokio::test]
async fn a_hot_worker_rests_and_the_topology_says_why() {
    // Enough for the whole model, including the planner's 20% head-room
    // reserve, so the cool worker can carry it alone.
    let ram = 8 * 1024 * 1024 * 128 * 5 / 4;
    let (hot_port, _hot_handle) = serve_mock(
        MockWorker::new("hot", 7, 10, Arc::new(Mutex::new(Vec::new()))).with_health(80.0, 46.0),
    )
    .await;
    let (cool_port, _cool_handle) = serve_mock(
        MockWorker::new("cool", 7, 10, Arc::new(Mutex::new(Vec::new()))).with_health(80.0, 30.0),
    )
    .await;

    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("hot", hot_port, ram));
    registry.register_or_update(node("cool", cool_port, ram));

    let store = Arc::new(Store::open_in_memory().expect("store"));
    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 8,
            weight_size_bytes: 8 * 1024 * 1024 * 128,
            local_file_path: "/models/llama.gguf".into(),
        })
        .expect("model");

    let cache = Arc::new(TelemetryCache::new());
    let state = AppState::new(
        "cluster-1".into(),
        registry,
        store,
        Arc::new(StubBackend::new()),
        TelemetryBus::new(64),
        HostInfo {
            node_id: "master".into(),
            device_name: "Master".into(),
            // Small on purpose: the master can hold one layer, so the workers
            // have to carry the rest.
            total_ram_mb: 256,
            cpu_cores: 4,
        },
    )
    .with_worker_transports(Arc::new(PlaintextTransports))
    .with_telemetry_cache(cache.clone());

    // One poll cycle: the workers report their real temperature.
    poll_once(
        &state.registry,
        &PlaintextTransports,
        &state.telemetry,
        &cache,
        &FixedHostMemory(42.0),
    )
    .await;

    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/cluster/topology")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");

    // The measured temperatures reach the response.
    let thermal = body["thermal"].as_array().expect("thermal array");
    let hot = thermal
        .iter()
        .find(|t| t["node_id"] == "hot")
        .expect("the hot worker is reported");
    assert_eq!(hot["temp_celsius"], 46.0);
    assert_eq!(hot["capped"], true, "the hot worker is capped");
    let cool = thermal
        .iter()
        .find(|t| t["node_id"] == "cool")
        .expect("the cool worker is reported");
    assert_eq!(cool["temp_celsius"], 30.0);
    assert_eq!(cool["capped"], false, "a cool worker is never capped");

    // Resting was enough, so the cap never had to be relaxed.
    assert_eq!(body["thermal_relaxed"], false);

    // And the effect is visible: the hot worker holds no layers.
    let nodes = body["nodes"].as_array().expect("nodes");
    let hot_node = nodes
        .iter()
        .find(|n| n["node_id"] == "hot")
        .expect("the hot worker is in the topology");
    assert!(
        hot_node["assigned_layers"]
            .as_array()
            .expect("assigned layers")
            .is_empty(),
        "the hot worker must rest, got {:?}",
        hot_node["assigned_layers"]
    );
    let cool_node = nodes
        .iter()
        .find(|n| n["node_id"] == "cool")
        .expect("the cool worker is in the topology");
    assert!(
        !cool_node["assigned_layers"]
            .as_array()
            .expect("assigned layers")
            .is_empty(),
        "the cool worker carries the load"
    );

    // The master has no measurement, so it is never capped and never listed.
    assert!(
        !thermal.iter().any(|t| t["node_id"] == "master"),
        "the master is not a polled worker"
    );
}

/// The cap is not decoration: when the cluster genuinely needs the hot
/// worker, the plan says so (spec D-F3).
///
/// This is the API-level guard for the cap itself: with the cap removed, the
/// hot worker would take the overflow at the first (resting) step and the
/// plan would never report a relaxation — so this test fails.
#[tokio::test]
async fn a_cluster_that_needs_the_hot_worker_reports_the_relaxation() {
    let (hot_port, _hot_handle) = serve_mock(
        MockWorker::new("hot", 7, 10, Arc::new(Mutex::new(Vec::new()))).with_health(80.0, 46.0),
    )
    .await;
    // A second worker with too little RAM to hold anything: the cluster
    // cannot place the model without the hot one.
    let (small_port, _small_handle) = serve_mock(
        MockWorker::new("small", 7, 10, Arc::new(Mutex::new(Vec::new()))).with_health(80.0, 30.0),
    )
    .await;

    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("hot", hot_port, 8 * 1024 * 1024 * 128 * 5 / 4));
    registry.register_or_update(node("small", small_port, 1024 * 1024));

    let store = Arc::new(Store::open_in_memory().expect("store"));
    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 8,
            weight_size_bytes: 8 * 1024 * 1024 * 128,
            local_file_path: "/models/llama.gguf".into(),
        })
        .expect("model");

    let cache = Arc::new(TelemetryCache::new());
    let state = AppState::new(
        "cluster-1".into(),
        registry,
        store,
        Arc::new(StubBackend::new()),
        TelemetryBus::new(64),
        HostInfo {
            node_id: "master".into(),
            device_name: "Master".into(),
            total_ram_mb: 256,
            cpu_cores: 4,
        },
    )
    .with_worker_transports(Arc::new(PlaintextTransports))
    .with_telemetry_cache(cache.clone());

    poll_once(
        &state.registry,
        &PlaintextTransports,
        &state.telemetry,
        &cache,
        &FixedHostMemory(42.0),
    )
    .await;

    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/cluster/topology")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");

    assert_eq!(
        body["thermal_relaxed"], true,
        "resting was not an option, so the cap had to be relaxed"
    );
    let hot = body["thermal"]
        .as_array()
        .expect("thermal array")
        .iter()
        .find(|t| t["node_id"] == "hot")
        .expect("the hot worker is reported")
        .clone();
    assert_eq!(hot["temp_celsius"], 46.0);
    assert_eq!(
        hot["capped"], false,
        "a worker that took what it must is not reported as capped"
    );
}
