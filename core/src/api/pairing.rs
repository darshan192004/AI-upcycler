//! QR/PIN pairing REST endpoints (`docs/security.md` §3).
//!
//! `POST /api/pairing/session` starts a single-use PIN session and returns the
//! QR payload (CA fingerprint + PIN). `POST /api/pairing/enroll` redeems the
//! PIN and issues the worker's mTLS identities — a server certificate for the
//! master→worker links the split driver dials, and a client certificate for
//! worker→master — then records the enrollment against the declared `node_id`.
//! Both live on the loopback-only local API.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::{ApiError, ApiResult, AppState};
use crate::security::{is_valid_certificate_address, PairingError};

/// Subject alternative names used when a worker declares none: the ADB tunnel
/// makes the master dial the phone's own loopback (`adb reverse`).
const DEFAULT_ADDRESSES: [&str; 2] = ["127.0.0.1", "localhost"];

/// Upper bound on declared addresses, so one enrollment cannot mint a
/// certificate with an unbounded SAN list.
const MAX_ADDRESSES: usize = 8;

#[derive(Debug, Serialize)]
pub struct PairingSessionResponse {
    pub pin: String,
    pub qr_payload: String,
    pub expires_in_secs: u64,
    pub fingerprint: String,
}

pub async fn create_session(
    State(state): State<AppState>,
) -> ApiResult<Json<PairingSessionResponse>> {
    let fingerprint = state.security.ca.fingerprint().to_string();
    let (pin, qr_payload, expires_in_secs) = state.security.pairing.create_session(&fingerprint);
    Ok(Json(PairingSessionResponse {
        pin,
        qr_payload,
        expires_in_secs,
        fingerprint,
    }))
}

#[derive(Debug, Deserialize)]
pub struct EnrollRequest {
    pub pin: String,
    pub device_name: String,
    /// The `node_id` the master dials this worker by (`DiscoveredNode::node_id`
    /// for mDNS, `adb:<serial>` for USB). Required in practice: enrollment and
    /// the split driver's credential lookup are both keyed by it, and defaulting
    /// it would silently disagree with what discovery reports.
    #[serde(default)]
    pub node_id: String,
    /// Addresses the master will dial this worker at. They become the subject
    /// alternative names of the issued server certificate; omitting them falls
    /// back to the ADB-tunnel loopback pair.
    #[serde(default)]
    pub addresses: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct EnrollResponse {
    pub ca_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
    /// The worker's TLS server identity: what it serves `WorkerService` behind.
    /// The master validates it against `fingerprint` on every dial.
    pub server_cert_pem: String,
    pub server_key_pem: String,
    pub fingerprint: String,
}

pub async fn enroll(
    State(state): State<AppState>,
    Json(req): Json<EnrollRequest>,
) -> ApiResult<Json<EnrollResponse>> {
    let device_name = req.device_name.trim();
    if device_name.is_empty() || device_name.chars().count() > 64 {
        return Err(ApiError::BadRequest(
            "device_name must be 1-64 characters".into(),
        ));
    }
    let node_id = req.node_id.trim();
    if node_id.is_empty() || node_id.chars().count() > 128 {
        return Err(ApiError::BadRequest(
            "node_id must be 1-128 characters".into(),
        ));
    }
    let addresses = declared_addresses(req.addresses)?;

    // The PIN is single-use, so only spend it once the request is known good.
    state
        .security
        .pairing
        .redeem(req.pin.trim())
        .map_err(map_pairing_error)?;

    let client = state
        .security
        .ca
        .issue_client_cert(device_name)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let server = state
        .security
        .ca
        .issue_server_cert(&addresses)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    state
        .security
        .enrollments
        .enroll(node_id, device_name, addresses);

    Ok(Json(EnrollResponse {
        ca_pem: state.security.ca.ca_pem(),
        client_cert_pem: client.cert_pem,
        client_key_pem: client.key_pem,
        server_cert_pem: server.cert_pem,
        server_key_pem: server.key_pem,
        fingerprint: state.security.ca.fingerprint().to_string(),
    }))
}

/// Validates the worker's declared addresses, defaulting to the ADB loopback
/// pair when the request omits them.
fn declared_addresses(declared: Option<Vec<String>>) -> ApiResult<Vec<String>> {
    let Some(declared) = declared else {
        return Ok(DEFAULT_ADDRESSES.iter().map(|a| a.to_string()).collect());
    };
    let addresses: Vec<String> = declared
        .into_iter()
        .map(|address| address.trim().to_string())
        .collect();
    if addresses.is_empty() {
        return Err(ApiError::BadRequest(
            "addresses must list at least one address, or be omitted entirely".into(),
        ));
    }
    if addresses.len() > MAX_ADDRESSES {
        return Err(ApiError::BadRequest(format!(
            "at most {MAX_ADDRESSES} addresses are allowed"
        )));
    }
    for address in &addresses {
        if !is_valid_certificate_address(address) {
            return Err(ApiError::BadRequest(format!(
                "invalid address {address:?}: expected an IP address or a DNS name"
            )));
        }
    }
    Ok(addresses)
}

fn map_pairing_error(e: PairingError) -> ApiError {
    match e {
        PairingError::AlreadyUsed => ApiError::Conflict(e.to_string()),
        PairingError::Credential(msg) => ApiError::Internal(msg),
        PairingError::InvalidPin | PairingError::Expired => ApiError::Forbidden(e.to_string()),
    }
}
