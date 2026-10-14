use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Error type mapped onto the OpenAI-style error envelope used by the API.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Forbidden(String),
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
    #[error("{0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::Store(_) | ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = Json(json!({
            "error": {
                "message": self.to_string(),
                "type": status.canonical_reason().unwrap_or("error").to_lowercase().replace(' ', "_"),
                "code": status.as_u16(),
            }
        }));
        (status, body).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_statuses_to_envelope() {
        let response = ApiError::NotFound("model x".into()).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = ApiError::BadRequest("nope".into()).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = ApiError::Conflict("busy".into()).into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let response = ApiError::Forbidden("bad pin".into()).into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
