//! Credential-free provider status, key management, and connectivity checks.
//!
//! Secret values are written straight to the injected [`SecretStore`] and are
//! never copied into a response, a log line, or the telemetry bus. Only the
//! boolean `configured` flag and a short text preview are exposed here.

use axum::extract::{Path, State};
use axum::Json;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::api::backend::{ChatMessage, GenerationRequest};
use crate::api::{ApiError, ApiResult, AppState};
use crate::providers::{bare_model, descriptor_for, CloudRoute, ProviderId, ProviderStatus};
use crate::secrets::SecretScope;

/// Maximum characters of a connectivity-check reply returned to the dashboard.
const PREVIEW_LIMIT: usize = 240;

/// The provider list serializes as a bare array, which is what the dashboard
/// consumes. The newtype keeps the handler signature named in the plan.
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct ProviderList(pub Vec<ProviderStatus>);

/// A credential write body. The value is consumed by the secret store and is
/// deliberately absent from every `Debug`/`Serialize` surface in this module.
#[derive(Debug, Deserialize)]
pub struct SecretValue {
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct ProviderTestRequest {
    pub model: String,
}

#[derive(Debug, Serialize)]
pub struct ProviderTestResult {
    pub provider: ProviderId,
    pub model: String,
    pub ok: bool,
    pub preview: String,
}

fn parse_provider(id: &str) -> ApiResult<ProviderId> {
    ProviderId::parse(id).ok_or_else(|| ApiError::NotFound(format!("provider '{id}'")))
}

fn provider_status(state: &AppState, provider: ProviderId) -> ApiResult<Json<ProviderStatus>> {
    state
        .providers
        .statuses()
        .into_iter()
        .find(|status| status.id == provider)
        .map(Json)
        .ok_or_else(|| ApiError::Internal("provider descriptor is unavailable".into()))
}

/// Lists all known providers and their capabilities without any credential.
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<ProviderList>> {
    Ok(Json(ProviderList(state.providers.statuses())))
}

/// Stores a provider credential in the OS keychain and returns the refreshed
/// credential-free status. Store errors are reduced to fixed strings: a native
/// error may echo bytes of the value it was given.
pub async fn put_secret(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SecretValue>,
) -> ApiResult<Json<ProviderStatus>> {
    let provider = parse_provider(&id)?;
    if body.value.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "secret value must not be empty".into(),
        ));
    }
    state
        .secrets
        .set(SecretScope::Provider(provider), &body.value)
        .map_err(|_| ApiError::Internal("the OS secret store rejected the write".into()))?;
    provider_status(&state, provider)
}

/// Deletes a provider credential. Deleting an absent key is a successful
/// no-op, so the dashboard's reset action is idempotent.
pub async fn delete_secret(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ProviderStatus>> {
    let provider = parse_provider(&id)?;
    state
        .secrets
        .delete(SecretScope::Provider(provider))
        .map_err(|_| ApiError::Internal("the OS secret store rejected the delete".into()))?;
    provider_status(&state, provider)
}

/// Sends one short, text-only turn through the provider and returns a bounded
/// preview. This is a connectivity check, not a chat endpoint: it never writes
/// a session and its reply is truncated before it reaches the caller.
pub async fn test(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ProviderTestRequest>,
) -> ApiResult<Json<ProviderTestResult>> {
    let provider = parse_provider(&id)?;
    let model = bare_model(body.model.trim()).to_string();
    if model.is_empty()
        || !descriptor_for(provider)
            .models
            .iter()
            .any(|candidate| candidate.id == model)
    {
        return Err(ApiError::BadRequest(format!(
            "model '{}' is not available for provider '{}'",
            body.model,
            provider.as_str()
        )));
    }

    let configured = state
        .providers
        .statuses()
        .iter()
        .any(|status| status.id == provider && status.configured);
    if !configured {
        return Err(ApiError::BadRequest(format!(
            "{} is not configured; save its API key in Settings before testing",
            provider.as_str()
        )));
    }

    let route = CloudRoute {
        provider,
        model: model.clone(),
    };
    let request = GenerationRequest {
        model: model.clone(),
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "Reply with the single word: ok".into(),
            ..Default::default()
        }],
        max_tokens: 32,
        ..Default::default()
    };

    let mut preview = String::new();
    let mut failed = false;
    let mut stream = state.providers.stream(&route, request);
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) if preview.len() < PREVIEW_LIMIT => {
                let take = PREVIEW_LIMIT - preview.len();
                preview.extend(chunk.content.chars().take(take));
            }
            Ok(_) => {}
            // Upstream error text can echo request bytes. Report only that the
            // check failed and which provider was involved.
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    if failed {
        return Err(ApiError::BadRequest(format!(
            "{} test failed; check its API key and base URL",
            provider.as_str()
        )));
    }

    Ok(Json(ProviderTestResult {
        provider,
        model,
        ok: true,
        preview,
    }))
}
