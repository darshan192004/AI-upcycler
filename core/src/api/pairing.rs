//! QR/PIN pairing REST endpoints (`docs/security.md` §3).
//!
//! `POST /api/pairing/session` starts a single-use PIN session and returns the
//! QR payload (CA fingerprint + PIN). `POST /api/pairing/enroll` redeems the
//! PIN and issues a worker client certificate signed by the master's
//! ephemeral CA. Both live on the loopback-only local API.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::{ApiError, ApiResult, AppState};
use crate::security::PairingError;

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
}

#[derive(Debug, Serialize)]
pub struct EnrollResponse {
    pub ca_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
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
    state
        .security
        .pairing
        .redeem(req.pin.trim())
        .map_err(map_pairing_error)?;
    let identity = state
        .security
        .ca
        .issue_client_cert(device_name)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(EnrollResponse {
        ca_pem: state.security.ca.ca_pem(),
        client_cert_pem: identity.cert_pem,
        client_key_pem: identity.key_pem,
        fingerprint: state.security.ca.fingerprint().to_string(),
    }))
}

fn map_pairing_error(e: PairingError) -> ApiError {
    match e {
        PairingError::AlreadyUsed => ApiError::Conflict(e.to_string()),
        PairingError::Credential(msg) => ApiError::Internal(msg),
        PairingError::InvalidPin | PairingError::Expired => ApiError::Forbidden(e.to_string()),
    }
}
