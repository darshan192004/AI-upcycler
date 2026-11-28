//! Non-secret settings, recommendations and public config HTTP surface
//! (sub-project D task 6).
//!
//! Secrets live in the OS keyring behind `/api/settings/secrets/:provider`
//! (handled by `providers`). Nothing in this module reads or serialises that
//! store: the settings body is a strict, deny-unknown-fields value object.

use crate::api::{ApiError, ApiResult, AppState};
use crate::autoconfig::{
    recommend_settings, AppMemoryProbe, ModelLayerInfo, SettingsRecommendation, TelemetryCacheProbe,
};
use crate::settings::AppSettings;
use axum::extract::rejection::JsonRejection;
use axum::{extract::State, Json};
use serde::Serialize;

/// The only public config fact: whether the multimodal gate is on.
///
/// Deliberately a one-field struct — no settings, paths, keys, tokens or
/// provider state can be added here without a compile-time contract change.
#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    pub multimodal_enabled: bool,
}

/// `GET /api/config`: read by the desktop client's fail-closed multimodal gate.
pub async fn config(State(state): State<AppState>) -> Json<ConfigResponse> {
    Json(ConfigResponse {
        multimodal_enabled: state.multimodal_enabled,
    })
}

/// `GET /api/settings`: prefers stored values, falling back to app defaults.
pub async fn get(State(state): State<AppState>) -> ApiResult<Json<AppSettings>> {
    Ok(Json(state.store.get_settings()?))
}

/// `PUT /api/settings`: validates before persisting and echoes only the
/// accepted value object. Unknown fields (including secret-shaped ones) are
/// rejected with a 400 in the shared error envelope.
pub async fn put(
    State(state): State<AppState>,
    body: Result<Json<AppSettings>, JsonRejection>,
) -> ApiResult<Json<AppSettings>> {
    let Json(update) = body.map_err(bad_request_from_rejection)?;
    update
        .validate()
        .map_err(|message| ApiError::BadRequest(message.into()))?;
    state.store.put_settings(&update)?;
    Ok(Json(update))
}

/// `GET /api/settings/recommendations`: derives limits from injected memory,
/// worker health and local model metadata. Virtual (cloud-only) catalog rows
/// have no GGUF layers to reserve RAM for, so they are excluded.
pub async fn recommendations(
    State(state): State<AppState>,
) -> ApiResult<Json<SettingsRecommendation>> {
    let models: Vec<ModelLayerInfo> = state
        .store
        .list_models()?
        .into_iter()
        .filter(|model| {
            !model
                .local_file_path
                .starts_with(crate::store::VIRTUAL_MODEL_PATH_PREFIX)
        })
        .map(|model| ModelLayerInfo {
            model_id: model.model_id,
            total_layers: u32::try_from(model.total_layers).unwrap_or(0),
        })
        .collect();
    let memory = AppMemoryProbe::new(state.host_memory.clone());
    let workers = TelemetryCacheProbe::new(state.telemetry_cache.clone());
    Ok(Json(recommend_settings(&memory, &workers, &models)))
}

/// Converts an axum JSON rejection into the API's 400 envelope.
///
/// Serde's free text is never rendered: a wrong-typed caller value can contain
/// a marker substring ("invalid value") or an escaped quote, so even stripping
/// quoted fragments can echo secrets. Only fixed, structural classifications
/// are returned; every other rejection is a generic `invalid request body`.
pub(crate) fn bad_request_from_rejection(rejection: JsonRejection) -> ApiError {
    let text = rejection.body_text();
    let detail = text
        .strip_prefix("Failed to deserialize the JSON body into the target type: ")
        .unwrap_or(&text);
    let detail = detail
        .split_once(" at line ")
        .map(|(head, _)| head)
        .unwrap_or(detail);
    let message = if detail.contains("unknown field") {
        "unknown field"
    } else if detail.contains("missing field") {
        "missing field"
    } else {
        "invalid request body"
    };
    ApiError::BadRequest(message.into())
}
