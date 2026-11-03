//! End-to-end tests for the device onboarding endpoints.
//!
//! A `FakeAdb` stands in for the phone, so the whole sequence — discover,
//! connect, install, revoke — runs in CI with no device, no cable, and no
//! subprocess.

use ai_upcycler_core::adb::wireless::{AdbControl, ConnectOutcome, DeviceEntry};
use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::lan::{LanDiscovery, Sighting, Transport};
use ai_upcycler_core::discovery::mdns::NodeRegistry;
use ai_upcycler_core::onboarding::OnboardingService;
use ai_upcycler_core::store::Store;
use ai_upcycler_core::telemetry::TelemetryBus;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

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

async fn call(app: Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
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
