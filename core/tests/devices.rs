//! End-to-end tests for the device onboarding endpoints.
//!
//! A `FakeAdb` stands in for the phone, so the whole sequence — discover,
//! connect, install, revoke — runs in CI with no device, no cable, and no
//! subprocess.

use ai_upcycler_core::adb::wireless::{AdbControl, ConnectOutcome, DeviceEntry};
use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::lan::{LanDiscovery, Sighting, Transport};
use ai_upcycler_core::discovery::mdns::{DiscoveredNode, NodeRegistry};
use ai_upcycler_core::onboarding::OnboardingService;
use ai_upcycler_core::rpc::engine::{WorkerTransport, WorkerTransportSource};
use ai_upcycler_core::store::{NewModel, Store};
use ai_upcycler_core::telemetry::TelemetryBus;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

mod common;

use common::{serve_mock, Events, MockWorker};

/// Loopback plaintext for mock workers (spec 2026-10-21, D-C3): they are not
/// enrolled as TLS peers, while the enrollment rule itself (spec
/// 2026-11-03-model-provisioning, D-P9) is asserted by the handlers.
struct PlaintextTransports;

impl WorkerTransportSource for PlaintextTransports {
    fn transport_for(&self, _node_id: &str) -> Result<WorkerTransport, String> {
        Ok(WorkerTransport::Plaintext)
    }
}

#[derive(Default)]
struct FakeAdb {
    version: Mutex<Option<String>>,
    installs: Mutex<Vec<String>>,
    disconnected: Mutex<Vec<String>>,
}

impl AdbControl for FakeAdb {
    fn connect(&self, _addr: &str) -> ConnectOutcome {
        ConnectOutcome::Connected
    }

    fn disconnect(&self, addr: &str) -> Result<(), String> {
        self.disconnected.lock().unwrap().push(addr.to_string());
        Ok(())
    }

    fn tcpip(&self, _serial: &str, _port: u16) -> Result<(), String> {
        Ok(())
    }

    fn install(&self, apk: &str) -> Result<(), String> {
        self.installs.lock().unwrap().push(apk.to_string());
        // A real install replaces the old package, so the version check after
        // it must see the new one.
        *self.version.lock().unwrap() = Some("0.1.0".to_string());
        Ok(())
    }

    fn version_name(&self) -> Result<Option<String>, String> {
        Ok(self.version.lock().unwrap().clone())
    }

    fn devices(&self) -> Result<Vec<DeviceEntry>, String> {
        Ok(Vec::new())
    }
}

fn state(fake: FakeAdb) -> AppState {
    let discovery = Arc::new(LanDiscovery::new());
    discovery.upsert(Sighting {
        key: "16284c49".into(),
        display_name: "Redmi".into(),
        address: "192.168.1.13:5555".into(),
        transport: Transport::Usb,
        authorized: Some(true),
    });

    let service =
        OnboardingService::new(Arc::new(fake), discovery, "0.1.0").with_worker_apk("worker.apk");

    AppState::new(
        "cluster-1".into(),
        Arc::new(NodeRegistry::new()),
        Arc::new(Store::open_in_memory().expect("store")),
        Arc::new(StubBackend::new()),
        TelemetryBus::new(8),
        HostInfo::default(),
    )
    .with_onboarding(Some(Arc::new(service)))
}

async fn respond(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        // Error responses are plain text, not JSON; keep them readable instead
        // of panicking in the helper.
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, body)
}

async fn call(app: Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    respond(app.oneshot(request).await.unwrap()).await
}

async fn post_json(app: Router, uri: &str, body: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    respond(app.oneshot(request).await.unwrap()).await
}

/// Drives the PIN hand-off the way the dashboard does: mint a session for the
/// serial, then redeem it as the phone would over its own ADB tunnel.
async fn pair(app: &Router, serial: &str, node_id: &str) {
    let (status, body) = call(
        app.clone(),
        "POST",
        &format!("/api/pairing/session?serial={serial}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let pin = body["pin"].as_str().expect("pin").to_string();
    let (status, body) = post_json(
        app.clone(),
        "/api/pairing/enroll",
        &format!(r#"{{"pin":"{pin}","device_name":"Redmi","node_id":"{node_id}"}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Puts a node in the registry under `node_id`, dialed at loopback `port`.
fn register_node(state: &AppState, node_id: &str, port: u16) {
    state.registry.upsert(DiscoveredNode {
        node_id: node_id.into(),
        device_name: "Redmi".into(),
        ip_address: "127.0.0.1".into(),
        port,
        total_ram_bytes: 8 * 1024 * 1024 * 1024,
        free_ram_bytes: 4 * 1024 * 1024 * 1024,
        connection_type: "WIFI_MDNS".into(),
        latency_ms: 1.0,
        last_seen_epoch_ms: 0,
    });
}

/// Registers a model row whose file lives at `path`. The path comes from the
/// store, never from a request body (spec 2026-11-03-model-provisioning,
/// D-P10).
fn register_model(state: &AppState, model_id: &str, path: &std::path::Path) {
    state
        .store
        .upsert_model(&NewModel {
            model_id: model_id.into(),
            file_name: format!("{model_id}.gguf"),
            quantization_format: "Q4_K_M".into(),
            total_layers: 28,
            weight_size_bytes: 4096,
            local_file_path: path.to_string_lossy().into_owned(),
        })
        .expect("model row");
}

/// A throwaway GGUF on disk, with its own directory so tests cannot collide.
fn temp_gguf(contents: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ai-upcycler-provision-http-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("tiny.gguf");
    std::fs::write(&path, contents).expect("write");
    path
}

#[tokio::test]
async fn the_whole_sequence_runs_from_discovered_to_enrolled_and_back() {
    let app = router(state(FakeAdb::default()));

    // 1. Discovery sees the phone but nothing has been tried.
    let (status, body) = call(app.clone(), "GET", "/api/devices/available").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["devices"][0]["state"]["state"], "discovered");

    // 2. Connect: adb is reachable, but there is no worker app.
    let (status, body) = call(app.clone(), "POST", "/api/devices/16284c49/connect").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"]["state"], "needs_install");
    assert_eq!(body["state"]["available"], "0.1.0");

    // 3. Install is an explicit action and re-checks the version.
    let (status, body) = call(app.clone(), "POST", "/api/devices/16284c49/install").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["state"]["state"], "needs_enrol",
        "installed is not enrolled"
    );

    // 4. Revoking a device that never enrolled is idempotent, not an error.
    let (status, body) = call(app.clone(), "DELETE", "/api/devices/16284c49").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revoked"], false);
}

#[tokio::test]
async fn revoking_an_enrolled_device_removes_the_enrollment() {
    let state = state(FakeAdb::default());
    state
        .security
        .enrollments
        .enroll("16284c49", "Redmi", vec!["192.168.1.13".into()]);
    let app = router(state);

    let (status, body) = call(app.clone(), "DELETE", "/api/devices/16284c49").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revoked"], true);
}

#[tokio::test]
async fn revoking_works_even_when_adb_is_missing() {
    // Taking a device out of the cluster must not depend on the control-plane
    // binary: a build with no platform-tools can still revoke.
    let state = AppState::new(
        "cluster-1".into(),
        Arc::new(NodeRegistry::new()),
        Arc::new(Store::open_in_memory().expect("store")),
        Arc::new(StubBackend::new()),
        TelemetryBus::new(8),
        HostInfo::default(),
    );
    state
        .security
        .enrollments
        .enroll("phone-a", "Pixel 8", vec!["10.0.0.5".into()]);
    let app = router(state);

    let (status, body) = call(app, "DELETE", "/api/devices/phone-a").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revoked"], true);
}

#[tokio::test]
async fn onboarding_endpoints_say_why_they_are_off_when_adb_is_missing() {
    // A build without platform-tools still serves the cluster; it just cannot
    // touch devices, and says so instead of failing somewhere confusing.
    let app = router(AppState::new(
        "cluster-1".into(),
        Arc::new(NodeRegistry::new()),
        Arc::new(Store::open_in_memory().expect("store")),
        Arc::new(StubBackend::new()),
        TelemetryBus::new(8),
        HostInfo::default(),
    ));

    let (status, _) = call(app, "GET", "/api/devices/available").await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn a_scan_requires_an_explicit_subnet() {
    let app = router(state(FakeAdb::default()));

    let (status, _) = call(app, "POST", "/api/devices/scan?subnet=192.168").await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a sweep is a port scan; it must name the network it scans"
    );
}

// ---- model provisioning (spec 2026-11-03-model-provisioning) ---------------

#[tokio::test]
async fn provisioning_an_unknown_device_is_404() {
    // D-P10: the node comes from the registry, so a device that is not in it
    // has nothing to push to. Pinned against the plan's 409 — the spec is the
    // authority, and this asserts the reason, not just the status.
    let state = state(FakeAdb::default());
    register_model(&state, "tiny", &std::env::temp_dir().join("missing.gguf"));
    let app = router(state);

    let (status, body) = post_json(
        app,
        "/api/devices/ghost/provision",
        r#"{"model_id":"tiny"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        body.as_str()
            .unwrap_or_default()
            .contains("no cluster node is registered"),
        "{body}"
    );
}

#[tokio::test]
async fn provisioning_an_unknown_model_is_404() {
    let state = state(FakeAdb::default());
    register_node(&state, "16284c49", 50051);
    state
        .security
        .enrollments
        .enroll("16284c49", "Redmi", vec![]);
    let app = router(state);

    let (status, body) = post_json(
        app,
        "/api/devices/16284c49/provision",
        r#"{"model_id":"ghost"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        body.as_str().unwrap_or_default().contains("unknown model"),
        "{body}"
    );
}

#[tokio::test]
async fn provisioning_a_node_that_is_not_enrolled_is_409() {
    // D-P9: the push rides the mTLS data plane, so an unenrolled node is
    // refused before the file is even opened.
    let state = state(FakeAdb::default());
    register_node(&state, "16284c49", 50051);
    register_model(
        &state,
        "tiny",
        &std::env::temp_dir().join("no-such-model.gguf"),
    );
    let app = router(state);

    let (status, body) = post_json(
        app,
        "/api/devices/16284c49/provision",
        r#"{"model_id":"tiny"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.as_str().unwrap_or_default().contains("not enrolled"),
        "{body}"
    );
}

#[tokio::test]
async fn a_second_provision_while_running_is_409() {
    let state = state(FakeAdb::default());
    register_node(&state, "16284c49", 50051);
    state
        .security
        .enrollments
        .enroll("16284c49", "Redmi", vec![]);
    let file = temp_gguf(b"tiny model");
    register_model(&state, "tiny", &file);

    // Claim the device for a push that is already in flight instead of racing
    // a real transfer: the rule under test is the second POST's refusal.
    let provisioning =
        Arc::new(ai_upcycler_core::api::provisioning::ProvisioningRegistry::default());
    assert!(provisioning.try_begin("16284c49", 1024));
    let app = router(state.with_provisioning(provisioning));

    let (status, body) = post_json(
        app,
        "/api/devices/16284c49/provision",
        r#"{"model_id":"tiny"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.as_str()
            .unwrap_or_default()
            .contains("already running"),
        "{body}"
    );
    let _ = std::fs::remove_file(&file);
}

#[tokio::test]
async fn provision_progress_starts_idle() {
    let app = router(state(FakeAdb::default()));

    let (status, body) = call(app, "GET", "/api/devices/16284c49/provision").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "state": "idle" }));
}

#[tokio::test]
async fn enrolling_a_device_over_its_tunnel_marks_it_ready() {
    // The wireless-onboarding contract ("Enrolling → Ready"; Ready is claimed
    // only once the API layer completes the PIN hand-off): without this the
    // dashboard's Provision button would never appear on any card.
    let app = router(state(FakeAdb::default()));
    let (status, _) = call(app.clone(), "POST", "/api/devices/16284c49/connect").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(app.clone(), "POST", "/api/devices/16284c49/install").await;
    assert_eq!(status, StatusCode::OK);

    pair(&app, "16284c49", "Redmi._aiupcycler-worker._tcp.local.").await;

    let (status, body) = call(app, "GET", "/api/devices/available").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["devices"][0]["state"]["state"], "ready", "{body}");
    assert_eq!(
        body["devices"][0]["state"]["node_id"],
        "Redmi._aiupcycler-worker._tcp.local."
    );
}

#[tokio::test]
async fn a_model_push_runs_from_202_to_done() {
    // The slice end to end: the card's device id resolves to a registry node,
    // the enrollment rule is satisfied by the real PIN hand-off, the transfer
    // runs in its spawned task, and every byte reaches the worker.
    let worker = MockWorker::new("phone", 1, 10, Events::default());
    let push = worker.push_log();
    let (port, _server) = serve_mock(worker).await;

    let state = state(FakeAdb::default()).with_worker_transports(Arc::new(PlaintextTransports));
    register_node(&state, "Redmi._aiupcycler-worker._tcp.local.", port);
    let file = temp_gguf(b"the quick brown model");
    register_model(&state, "tiny", &file);

    let app = router(state);
    let (status, _) = call(app.clone(), "POST", "/api/devices/16284c49/connect").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(app.clone(), "POST", "/api/devices/16284c49/install").await;
    assert_eq!(status, StatusCode::OK);
    pair(&app, "16284c49", "Redmi._aiupcycler-worker._tcp.local.").await;

    let (status, body) = post_json(
        app.clone(),
        "/api/devices/16284c49/provision",
        r#"{"model_id":"tiny"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["state"], "running");
    assert_eq!(body["total_bytes"], 21u64, "{body}");

    // The transfer is a spawned task; poll until it settles (D-P8).
    let mut current = body;
    for _ in 0..400 {
        if current["state"] != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        let (status, next) = call(app.clone(), "GET", "/api/devices/16284c49/provision").await;
        assert_eq!(status, StatusCode::OK);
        current = next;
    }
    assert_eq!(current["state"], "done", "{current}");
    assert_eq!(current["bytes"], 21u64, "{current}");
    assert_eq!(
        push.lock().expect("push lock").bytes,
        b"the quick brown model"
    );
    let _ = std::fs::remove_file(&file);
}
