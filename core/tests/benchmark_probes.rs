//! End-to-end tests for the real benchmark probe path (spec 2026-10-21).
//!
//! The master probes each discovered node over the transport it actually
//! serves on (D-C3). These tests run in-proc mock workers over loopback
//! plaintext and assert the whole loop: the values recorded in `node_benchmark`
//! came from the wire, and a node that cannot answer is **counted, never
//! faked** (D-C6).
//!
//! Transport resolution is injected through `AppState::with_transports` — the
//! same seam `AdaptiveBackend` uses — because production resolves it from the
//! pairing enrollment registry, and a test cannot complete a pairing.

use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::{DiscoveredNode, NodeRegistry};
use ai_upcycler_core::rpc::engine::{WorkerTransport, WorkerTransportSource};
use ai_upcycler_core::security::SecurityState;
use ai_upcycler_core::store::Store;
use ai_upcycler_core::telemetry::TelemetryBus;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

mod common;

use common::{serve_mock, serve_mock_tls, MockWorker};

/// Plaintext for every node: loopback mock workers are not enrolled, and
/// production never dials an unenrolled node over plaintext.
struct PlaintextTransports;

impl WorkerTransportSource for PlaintextTransports {
    fn transport_for(&self, _node_id: &str) -> Result<WorkerTransport, String> {
        Ok(WorkerTransport::Plaintext)
    }
}

fn node(node_id: &str, port: u16) -> DiscoveredNode {
    node_at(node_id, "127.0.0.1", port)
}

fn node_at(node_id: &str, ip_address: &str, port: u16) -> DiscoveredNode {
    DiscoveredNode {
        node_id: node_id.into(),
        device_name: format!("device-{node_id}"),
        ip_address: ip_address.into(),
        port,
        total_ram_bytes: 8 * 1024 * 1024 * 1024,
        free_ram_bytes: 4 * 1024 * 1024 * 1024,
        connection_type: "USB_ADB".into(),
        latency_ms: 2.0,
        last_seen_epoch_ms: 0,
    }
}

/// Production wiring: no transport override, so the handler derives the dial
/// from `security` exactly as the desktop shell does.
fn base_state(registry: Arc<NodeRegistry>, store: Arc<Store>) -> AppState {
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

fn state(registry: Arc<NodeRegistry>, store: Arc<Store>) -> AppState {
    base_state(registry, store).with_worker_transports(Arc::new(PlaintextTransports))
}

async fn post_benchmark(app: Router) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/cluster/benchmark")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, serde_json::from_slice(&body).expect("json"))
}

/// The measured path (spec test 2): a worker answers with fixed values, and
/// the store row equals them exactly — a mutation check, because a row built
/// from a hash would differ from the wire.
#[tokio::test]
async fn records_the_values_the_worker_reported() {
    let (port, _handle) = serve_mock(MockWorker::new(
        "w1",
        7,
        10,
        Arc::new(Mutex::new(Vec::new())),
    ))
    .await;

    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("w1", port));
    let store = Arc::new(Store::open_in_memory().expect("store"));
    let app = router(state(registry, store.clone()));

    let (status, body) = post_benchmark(app).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "success");
    assert_eq!(body["nodes_tested"], 1);
    assert_eq!(body["nodes_failed"], 0);

    let rows = store.list_benchmarks("w1").expect("benchmarks");
    assert_eq!(rows.len(), 1, "exactly one measured row");
    // MockWorker's benchmark returns 1000.0 + tag, 100.0 + tag, 4.0 + tag
    // for tag 7 - the row must equal the wire, not a derivation.
    assert_eq!(rows[0].memory_bandwidth_mbs, 1007.0);
    assert_eq!(rows[0].throughput_flops, 107.0);
    // network_ping_ms stays the discovery latency (D-C4), not a probe timing.
    assert_eq!(rows[0].network_ping_ms, 2.0);
}

/// The unimplemented path (spec test 3): a worker that predates the probe RPC
/// answers `UNIMPLEMENTED`. It must be counted, write no row, and the endpoint
/// still returns 200.
#[tokio::test]
async fn counts_a_worker_that_cannot_measure() {
    let (port, _handle) = serve_mock(
        MockWorker::new("w1", 3, 10, Arc::new(Mutex::new(Vec::new()))).without_benchmark(),
    )
    .await;

    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("w1", port));
    let store = Arc::new(Store::open_in_memory().expect("store"));
    let app = router(state(registry, store.clone()));

    let (status, body) = post_benchmark(app).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "partial");
    assert_eq!(body["nodes_tested"], 0);
    assert_eq!(body["nodes_failed"], 1);
    assert_eq!(
        store.list_benchmarks("w1").expect("benchmarks").len(),
        0,
        "a node that could not be measured must not get a row"
    );
}

/// The unreachable path (spec test 4): nothing is listening. The probe fails,
/// no row is written, and the endpoint does not fail.
#[tokio::test]
async fn counts_a_worker_that_cannot_be_reached() {
    // Bind and drop, so the port is almost certainly free.
    let port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        listener.local_addr().expect("addr").port()
    };

    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("w1", port));
    let store = Arc::new(Store::open_in_memory().expect("store"));
    let app = router(state(registry, store.clone()));

    let (status, body) = post_benchmark(app).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["nodes_tested"], 0);
    assert_eq!(body["nodes_failed"], 1);
    assert_eq!(
        store.list_benchmarks("w1").expect("benchmarks").len(),
        0,
        "an unreachable node must not get a fabricated row"
    );
}

/// The mixed path (spec test 5): one measurable node and one that cannot
/// answer. One row, one failure counted, status `partial`.
#[tokio::test]
async fn mixes_measured_and_failed_nodes() {
    let (good_port, _good) = serve_mock(MockWorker::new(
        "good",
        1,
        10,
        Arc::new(Mutex::new(Vec::new())),
    ))
    .await;
    let (bad_port, _bad) = serve_mock(
        MockWorker::new("bad", 2, 10, Arc::new(Mutex::new(Vec::new()))).without_benchmark(),
    )
    .await;

    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("good", good_port));
    registry.register_or_update(node("bad", bad_port));
    let store = Arc::new(Store::open_in_memory().expect("store"));
    let app = router(state(registry, store.clone()));

    let (status, body) = post_benchmark(app).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "partial");
    assert_eq!(body["nodes_tested"], 1);
    assert_eq!(body["nodes_failed"], 1);
    assert_eq!(store.list_benchmarks("good").expect("rows").len(), 1);
    assert_eq!(store.list_benchmarks("bad").expect("rows").len(), 0);
}

/// The stub is test-only (spec test 6, D-C7): the production handler cannot
/// reach a hash-derived value. `StubBenchmarker` no longer exists outside
/// `#[cfg(test)]`, so a fabricated row is not merely unlikely — it is
/// unconstructible. This test pins the observable consequence: a node that
/// answers gets the *worker's* numbers, and only the worker's numbers.
#[tokio::test]
async fn the_recorded_value_comes_from_the_worker_not_from_a_derivation() {
    let (port, _handle) = serve_mock(MockWorker::new(
        "w1",
        42,
        10,
        Arc::new(Mutex::new(Vec::new())),
    ))
    .await;

    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("w1", port));
    let store = Arc::new(Store::open_in_memory().expect("store"));
    let app = router(state(registry, store.clone()));

    post_benchmark(app).await;

    let rows = store.list_benchmarks("w1").expect("benchmarks");
    // The old stub derived 400..1000 MB/s from an FNV-1a hash of the node id
    // and 5..55 FLOP/s. The mock's fixed 1042.0 / 142.0 fall outside both
    // ranges, so a stub-derived row could not produce them.
    assert_eq!(rows[0].memory_bandwidth_mbs, 1042.0);
    assert_eq!(rows[0].throughput_flops, 142.0);
    assert!(
        rows[0].memory_bandwidth_mbs > 1000.0,
        "the old stub's range was 400..1000 MB/s"
    );
    assert!(
        rows[0].throughput_flops > 55.0,
        "the old stub's range was 5..55 FLOP/s"
    );
}

/// The probe is one RPC per node and never a separate health round-trip
/// (D-C4): the mock records the calls it received.
#[tokio::test]
async fn probes_each_node_exactly_once() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (port, _handle) = serve_mock(MockWorker::new("w1", 9, 10, seen.clone())).await;

    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(node("w1", port));
    let store = Arc::new(Store::open_in_memory().expect("store"));
    let app = router(state(registry, store));

    post_benchmark(app).await;

    let log = seen.lock().expect("events lock").clone();
    assert_eq!(
        log.iter()
            .filter(|event| event.starts_with("benchmark:"))
            .count(),
        1,
        "exactly one benchmark RPC, no extra health round-trip: {log:?}"
    );
    assert_eq!(
        log.len(),
        1,
        "the probe was the only RPC the worker received: {log:?}"
    );
}

/// D-C3's other branch, and the regression test for `with_security`: an
/// enrolled node is dialed over mTLS against the *live* pairing CA. The shell
/// swaps a shared `SecurityState` in after construction, so a transport source
/// captured at construction time would hold a different CA, see this enrolled
/// node as unenrolled, and dial plaintext to a TLS-only port - failing here.
#[tokio::test]
async fn benchmarks_an_enrolled_worker_over_mtls_from_the_live_security_state() {
    let security = Arc::new(SecurityState::default());
    security.enrollments.enroll("w1", "phone", vec![]);

    let (port, _handle) = serve_mock_tls(
        MockWorker::new("w1", 5, 10, Arc::new(Mutex::new(Vec::new()))),
        &security.ca,
    )
    .await;

    let registry = Arc::new(NodeRegistry::new());
    // The test CA issues its server certificate for `localhost`.
    registry.register_or_update(node_at("w1", "localhost", port));
    let store = Arc::new(Store::open_in_memory().expect("store"));
    let app = router(base_state(registry, store.clone()).with_security(security));

    let (status, body) = post_benchmark(app).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["status"], "success",
        "enrolled node measured over mTLS"
    );
    assert_eq!(body["nodes_tested"], 1);
    assert_eq!(body["nodes_failed"], 0);

    let rows = store.list_benchmarks("w1").expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_bandwidth_mbs, 1005.0);
    assert_eq!(rows[0].throughput_flops, 105.0);
}
