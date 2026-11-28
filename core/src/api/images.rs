//! REST surface for generated images (sub-project F, task 7).
//!
//! Generation is delegated to [`crate::imagegen::registry::ImageBackendRegistry`],
//! bytes are persisted through the content-addressed media bridge, and only
//! metadata is written to SQLite. Upstream failures are reduced to fixed
//! literals so no provider response body, URL, header, or credential can reach
//! an API response.

use crate::api::{ApiError, ApiResult, AppState};
use crate::imagegen::media::{read_generated_image, store_generated_image, StoredGeneratedImage};
use crate::imagegen::{GenerateImageRequest, ImageError};
use crate::store::{ImageRow, NewImage};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

impl From<ImageError> for ApiError {
    fn from(error: ImageError) -> Self {
        // Every committed `ImageError` variant already carries a fixed literal
        // (or a fixed provider name), so this string never contains a request
        // body, URL, header, or secret value.
        let message = error.to_string();
        match error {
            ImageError::InvalidPrompt
            | ImageError::UnsupportedProvider
            | ImageError::LocalDisabled
            | ImageError::LocalEngineMissing
            | ImageError::ProviderNotConfigured(_) => ApiError::BadRequest(message),
            ImageError::UpstreamStatus(_)
            | ImageError::UpstreamBody(_)
            | ImageError::EmptyResponse
            | ImageError::UnsupportedFormat => ApiError::BadGateway(message),
        }
    }
}

/// Validates, generates, persists, and indexes one image.
pub async fn generate_image(
    State(state): State<AppState>,
    Json(request): Json<GenerateImageRequest>,
) -> ApiResult<(StatusCode, Json<ImageRow>)> {
    // Failures here happen before any media or SQLite write.
    let generation = state.image_backends.generate(&request).await?;
    let stored = store_generated_image(&state.media, &generation.generated).await?;
    let row = state.store.insert_image(&NewImage {
        image_id: Uuid::new_v4().to_string(),
        provider: generation.provider.as_str().to_string(),
        model: generation.model,
        prompt: request.prompt.trim().to_string(),
        media_sha256: stored.sha256_hex,
        media_path: stored.relative_path,
        mime_type: stored.mime_type,
        byte_len: stored.byte_len,
        size: request.size.as_str().to_string(),
    })?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// Returns only the indexed metadata for one image.
pub async fn get_image_metadata(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ImageRow>> {
    state
        .store
        .get_image(&id)?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("image {id} not found")))
}

/// Streams the stored bytes with immutable content-addressed response headers.
pub async fn get_image_bytes(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let row = state
        .store
        .get_image(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("image {id} not found")))?;
    let stored = StoredGeneratedImage {
        sha256_hex: row.media_sha256.clone(),
        relative_path: row.media_path.clone(),
        mime_type: row.mime_type.clone(),
        byte_len: row.byte_len,
    };
    let bytes = read_generated_image(&state.media, &stored)
        .await
        .map_err(|_| ApiError::Internal("generated image bytes are missing".to_string()))?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, row.mime_type),
            (header::ETAG, format!("\"{}\"", row.media_sha256)),
            (
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable".to_string(),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
pub struct ListImagesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ImageListResponse {
    pub images: Vec<ImageRow>,
}

/// Lists indexed image metadata, newest first.
pub async fn list_images(
    State(state): State<AppState>,
    Query(query): Query<ListImagesQuery>,
) -> ApiResult<Json<ImageListResponse>> {
    let limit = query.limit.unwrap_or(60);
    let offset = query.offset.unwrap_or(0);
    if limit < 1 {
        return Err(ApiError::BadRequest("limit must be at least 1".to_string()));
    }
    if limit > 200 {
        return Err(ApiError::BadRequest(
            "limit must be at most 200".to_string(),
        ));
    }
    if offset < 0 {
        return Err(ApiError::BadRequest(
            "offset must be non-negative".to_string(),
        ));
    }
    let images = state.store.list_images(limit, offset)?;
    Ok(Json(ImageListResponse { images }))
}
