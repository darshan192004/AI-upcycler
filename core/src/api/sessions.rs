//! Shared session surface for remote chat clients.
//!
//! Chat completions create sessions through `session_id`; these endpoints let
//! a phone or desktop client rediscover them, replay their messages, rename
//! them and delete them. Response types stay limited to the store's session
//! and message rows, so no secrets or file paths are added to the surface.

use crate::api::{ApiError, ApiResult, AppState};
use crate::store::{ChatMessageRow, ChatSessionRow};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct SessionList {
    pub sessions: Vec<ChatSessionRow>,
}

#[derive(Debug, Serialize)]
pub struct SessionMessages {
    pub messages: Vec<ChatMessageRow>,
}

/// Lists every stored session in `created_at, session_id` order (the store's
/// own ordering), with no message bodies attached.
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<SessionList>> {
    let sessions = state.store.list_sessions()?;
    Ok(Json(SessionList { sessions }))
}

/// Returns one session row. A missing id is a 404 in the shared error envelope.
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ChatSessionRow>> {
    let session = state
        .store
        .get_session(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("session {id} not found")))?;
    Ok(Json(session))
}

/// Returns the messages of one session in `message_id` order. The full
/// [`ChatMessageRow`] is serialised: content stays an ordinary user-facing
/// string, with no separate field for secrets or encrypted-looking payloads.
pub async fn messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionMessages>> {
    state
        .store
        .get_session(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("session {id} not found")))?;
    let messages = state.store.list_messages(&id)?;
    Ok(Json(SessionMessages { messages }))
}

/// The only writable field. Unknown fields are rejected so a credential can
/// never be smuggled into a session title.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameRequest {
    pub title: String,
}

/// `PUT /api/sessions/:id`: renames a session and returns the updated row.
pub async fn rename(
    Path(id): Path<String>,
    State(state): State<AppState>,
    body: Result<Json<RenameRequest>, JsonRejection>,
) -> ApiResult<Json<ChatSessionRow>> {
    let Json(request) = body.map_err(crate::api::settings::bad_request_from_rejection)?;
    let length = request.title.chars().count();
    if length == 0 || length > 120 || request.title.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "title must be 1..=120 characters".into(),
        ));
    }
    state
        .store
        .rename_session(&id, request.title.trim())?
        .ok_or_else(|| ApiError::NotFound(format!("session {id} not found")))
        .map(Json)
}

/// `DELETE /api/sessions/:id`: removes the session and its messages.
pub async fn delete(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<StatusCode> {
    if !state.store.delete_session(&id)? {
        return Err(ApiError::NotFound(format!("session {id} not found")));
    }
    Ok(StatusCode::NO_CONTENT)
}
