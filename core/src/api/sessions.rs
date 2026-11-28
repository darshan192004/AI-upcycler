//! Read-only shared session surface for remote chat clients.
//!
//! Chat completions create sessions through `session_id`; these endpoints only
//! let a phone or desktop client rediscover them and replay their messages.
//! Write routes are deliberately absent so this module can later gain `PUT`
//! and `DELETE` without renaming its response types.

use crate::api::{ApiError, ApiResult, AppState};
use crate::store::{ChatMessageRow, ChatSessionRow};
use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;

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
