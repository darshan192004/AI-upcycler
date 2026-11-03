//! HTTP surface for wireless device onboarding (spec D-A1..D-A5).
//!
//! Every handler that talks to a phone hops onto a blocking thread: `adb` is a
//! subprocess and `connect` may wait two minutes for Android's consent dialog.

use super::AppState;
use crate::discovery::lan::{
    sweep, Sighting, Transport, ADB_TCPIP_PORT, SWEEP_MAX_IN_FLIGHT, SWEEP_TIMEOUT,
};
use crate::onboarding::DeviceOnboarding;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

type ApiError = (StatusCode, String);

#[derive(Serialize)]
pub struct DevicesResponse {
    pub devices: Vec<DeviceOnboarding>,
}

#[derive(Deserialize)]
pub struct ScanQuery {
    /// The first three octets of the local network, e.g. `192.168.1`.
    pub subnet: String,
}

#[derive(Serialize)]
pub struct ScanResponse {
    pub found: usize,
    pub devices: Vec<DeviceOnboarding>,
}

#[derive(Serialize)]
pub struct RevokeResponse {
    /// Whether an enrollment was actually removed. `false` means the device was
    /// not enrolled, which is not a failure — revoking is idempotent.
    pub revoked: bool,
}

/// Onboarding is optional: a build with no adb still serves the cluster.
fn unavailable() -> ApiError {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "adb was not found, so device onboarding is off. Set AI_UPCYCLER_ADB, bundle \
         platform-tools, or put adb on PATH."
            .into(),
    )
}

fn join_failure(error: tokio::task::JoinError) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("device task failed: {error}"),
    )
}

/// `GET /api/devices/available` — everything discovery can see, with the state
/// onboarding has reached for each.
pub async fn available(State(state): State<AppState>) -> Result<Json<DevicesResponse>, ApiError> {
    let service = state.onboarding.clone().ok_or_else(unavailable)?;
    let devices = tokio::task::spawn_blocking(move || service.records())
        .await
        .map_err(join_failure)?;
    Ok(Json(DevicesResponse { devices }))
}

/// `POST /api/devices/{id}/connect`
pub async fn connect(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DeviceOnboarding>, ApiError> {
    let service = state.onboarding.clone().ok_or_else(unavailable)?;
    let record = tokio::task::spawn_blocking(move || service.connect(&id))
        .await
        .map_err(join_failure)?;
    Ok(Json(record))
}

/// `POST /api/devices/{id}/install`
pub async fn install(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DeviceOnboarding>, ApiError> {
    let service = state.onboarding.clone().ok_or_else(unavailable)?;
    let record = tokio::task::spawn_blocking(move || service.install(&id))
        .await
        .map_err(join_failure)?;
    Ok(Json(record))
}

/// `DELETE /api/devices/{id}` — revoke.
///
/// Removes the enrollment first, so a phone that keeps its ADB connection
/// still cannot serve: the transport rule refuses a worker with no enrollment.
pub async fn revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RevokeResponse>, ApiError> {
    let service = state.onboarding.clone().ok_or_else(unavailable)?;
    let security = state.security.clone();

    let revoked = tokio::task::spawn_blocking(move || {
        // Best effort: the device may already be gone from ADB, and that must
        // not stop the enrollment from being removed.
        let _ = service.disconnect(&id);
        security.enrollments.revoke(&id)
    })
    .await
    .map_err(join_failure)?;

    Ok(Json(RevokeResponse { revoked }))
}

/// `POST /api/devices/scan?subnet=192.168.1` — one opt-in sweep.
///
/// A sweep is a port scan. It is never started automatically, and the caller
/// has to name the network it wants scanned.
pub async fn scan(
    State(state): State<AppState>,
    Query(query): Query<ScanQuery>,
) -> Result<Json<ScanResponse>, ApiError> {
    let service = state.onboarding.clone().ok_or_else(unavailable)?;

    let subnet = query.subnet.trim().trim_end_matches('.').to_string();
    if subnet.split('.').count() != 3 {
        return Err((
            StatusCode::BAD_REQUEST,
            "subnet must be the first three octets of the network, e.g. 192.168.1".into(),
        ));
    }

    let discovery = service.discovery();
    let addresses = sweep(&subnet, ADB_TCPIP_PORT, SWEEP_TIMEOUT, SWEEP_MAX_IN_FLIGHT).await;

    for address in &addresses {
        discovery.upsert(Sighting {
            key: address.clone(),
            display_name: address.clone(),
            address: address.clone(),
            transport: Transport::WifiAdb,
            authorized: None,
        });
    }

    let found = addresses.len();
    let devices = tokio::task::spawn_blocking(move || service.records())
        .await
        .map_err(join_failure)?;
    Ok(Json(ScanResponse { found, devices }))
}
