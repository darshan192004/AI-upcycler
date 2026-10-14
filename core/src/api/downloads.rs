//! `POST /api/models/download` — asynchronous GGUF fetch from Hugging Face.
//!
//! The handler validates the request, answers `202 Accepted` immediately and
//! spawns a background runner that streams the file into the models directory,
//! fanning out `model_download_progress` telemetry events as bytes arrive. The
//! network transfer sits behind [`ModelFetcher`] so tests exercise the full
//! pipeline (write → progress → store registration) without touching the
//! internet.

use crate::api::{ApiError, ApiResult, AppState};
use crate::store::NewModel;
use crate::telemetry::{DownloadProgressData, DownloadStatus, TelemetryEvent};
use axum::extract::State;
use axum::Json;
use futures::stream::{BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// One streamed slice of a download, plus the total size when known.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadFrame {
    pub bytes: Vec<u8>,
    pub total_bytes: Option<u64>,
}

/// Minimum byte gap between `running` progress events; keeps small network
/// chunks from flooding the telemetry broadcast bus.
const PROGRESS_THROTTLE_BYTES: u64 = 1024 * 1024;

/// Upper bound of the GGUF header scanned for the layer (`block_count`) KV.
const GGUF_SCAN_CAP: u64 = 4 * 1024 * 1024;

/// Produces the byte stream for one Hugging Face repository file.
pub trait ModelFetcher: Send + Sync + 'static {
    fn fetch(
        &self,
        repo_id: String,
        file_name: String,
    ) -> BoxStream<'static, Result<DownloadFrame, String>>;
}

/// Real fetcher: streams `https://huggingface.co/<repo>/resolve/main/<file>`
/// over HTTPS (rustls), following the redirect Hugging Face issues to its CDN.
#[derive(Clone)]
pub struct HfFetcher {
    client: reqwest::Client,
}

impl Default for HfFetcher {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client builds without system dependencies");
        Self { client }
    }
}

impl ModelFetcher for HfFetcher {
    fn fetch(
        &self,
        repo_id: String,
        file_name: String,
    ) -> BoxStream<'static, Result<DownloadFrame, String>> {
        let url = match resolve_url(&repo_id, &file_name) {
            Ok(url) => url,
            Err(err) => return Box::pin(futures::stream::once(async move { Err(err) })),
        };
        let client = self.client.clone();
        Box::pin(async_stream::stream! {
            let response = match client.get(&url).send().await {
                Ok(response) => response,
                Err(err) => {
                    yield Err(format!("request failed: {err}"));
                    return;
                }
            };
            if !response.status().is_success() {
                yield Err(format!("hugging face responded with {}", response.status()));
                return;
            }
            let total = response.content_length();
            let mut stream = response.bytes_stream();
            while let Some(next) = stream.next().await {
                match next {
                    Ok(bytes) => yield Ok(DownloadFrame { bytes: bytes.to_vec(), total_bytes: total }),
                    Err(err) => {
                        yield Err(format!("stream failed: {err}"));
                        return;
                    }
                }
            }
        })
    }
}

/// Deterministic stand-in used by tests and offline builds: yields a tiny
/// well-formed GGUF header (16 blocks) split across two frames.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubFetcher;

impl StubFetcher {
    pub fn new() -> Self {
        Self
    }

    /// Header bytes: magic + version 3 + 0 tensors + one
    /// `llama.block_count = 16` metadata KV.
    pub fn gguf_fixture() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"GGUF");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        let key = b"llama.block_count";
        bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(&16i32.to_le_bytes());
        bytes
    }
}

impl ModelFetcher for StubFetcher {
    fn fetch(
        &self,
        _repo_id: String,
        _file_name: String,
    ) -> BoxStream<'static, Result<DownloadFrame, String>> {
        let all = Self::gguf_fixture();
        let (head, tail) = all.split_at(16);
        let total = all.len() as u64;
        Box::pin(futures::stream::iter(vec![
            Ok(DownloadFrame {
                bytes: head.to_vec(),
                total_bytes: Some(total),
            }),
            Ok(DownloadFrame {
                bytes: tail.to_vec(),
                total_bytes: Some(total),
            }),
        ]))
    }
}

/// Request body for `POST /api/models/download`.
#[derive(Debug, Deserialize)]
pub struct DownloadRequest {
    pub repo_id: String,
    pub file_name: String,
}

/// `202 Accepted` acknowledgement (the download itself runs in the background).
#[derive(Debug, Serialize)]
pub struct DownloadAccepted {
    pub status: &'static str,
    pub file_name: String,
}

pub async fn download(
    State(state): State<AppState>,
    Json(request): Json<DownloadRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<DownloadAccepted>)> {
    validate(&request.repo_id, &request.file_name)?;

    tokio::fs::create_dir_all(&state.models_dir)
        .await
        .map_err(|err| ApiError::Internal(format!("cannot create models directory: {err}")))?;

    {
        let mut active = state.active_downloads.lock().await;
        if !active.insert(request.file_name.clone()) {
            return Err(ApiError::Conflict(format!(
                "a download is already in progress for {}",
                request.file_name
            )));
        }
    }

    let file_name = request.file_name.clone();
    let model_id = model_id_from_file_name(&file_name);
    let dest: PathBuf = state.models_dir.join(&file_name);
    let repo_id = request.repo_id.clone();

    tokio::spawn(async move {
        run_download(&state, &repo_id, &model_id, &file_name, &dest).await;
        state.active_downloads.lock().await.remove(&file_name);
    });

    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(DownloadAccepted {
            status: "accepted",
            file_name: request.file_name,
        }),
    ))
}

/// Streams the file to `dest`, publishing progress; on success it registers
/// the model in the store so `/v1/models` and the UI pick it up.
async fn run_download(
    state: &AppState,
    repo_id: &str,
    model_id: &str,
    file_name: &str,
    dest: &Path,
) {
    let mut stream = state
        .fetcher
        .fetch(repo_id.to_string(), file_name.to_string());

    let mut file = match tokio::fs::File::create(dest).await {
        Ok(file) => file,
        Err(err) => {
            tracing::warn!("cannot create {}: {err}", dest.display());
            publish_progress(state, model_id, file_name, 0, 0, DownloadStatus::Error);
            return;
        }
    };

    let mut received: u64 = 0;
    let mut total: u64 = 0;
    let mut last_published: u64 = 0;
    publish_progress(state, model_id, file_name, 0, 0, DownloadStatus::Running);

    loop {
        match stream.next().await {
            Some(Ok(frame)) => {
                if total == 0 {
                    total = frame.total_bytes.unwrap_or(0);
                }
                if let Err(err) = file.write_all(&frame.bytes).await {
                    tracing::warn!("write to {} failed: {err}", dest.display());
                    drop(file);
                    let _ = tokio::fs::remove_file(dest).await;
                    publish_progress(
                        state,
                        model_id,
                        file_name,
                        received,
                        total,
                        DownloadStatus::Error,
                    );
                    return;
                }
                received = received.saturating_add(frame.bytes.len() as u64);
                let first_report = last_published == 0 && received > 0;
                if first_report || received - last_published >= PROGRESS_THROTTLE_BYTES {
                    last_published = received;
                    publish_progress(
                        state,
                        model_id,
                        file_name,
                        received,
                        total,
                        DownloadStatus::Running,
                    );
                }
            }
            Some(Err(err)) => {
                tracing::warn!("download of {file_name} failed: {err}");
                drop(file);
                let _ = tokio::fs::remove_file(dest).await;
                publish_progress(
                    state,
                    model_id,
                    file_name,
                    received,
                    total,
                    DownloadStatus::Error,
                );
                return;
            }
            None => break,
        }
    }

    if let Err(err) = file.flush().await {
        tracing::warn!("flush of {} failed: {err}", dest.display());
        drop(file);
        let _ = tokio::fs::remove_file(dest).await;
        publish_progress(
            state,
            model_id,
            file_name,
            received,
            total,
            DownloadStatus::Error,
        );
        return;
    }
    drop(file);

    let block_count = read_block_count_async(dest).await.unwrap_or(0);
    let new_model = NewModel {
        model_id: model_id.to_string(),
        file_name: file_name.to_string(),
        quantization_format: quantization_from_file_name(file_name),
        total_layers: i64::from(block_count),
        weight_size_bytes: i64::try_from(received).unwrap_or(i64::MAX),
        local_file_path: dest.to_string_lossy().into_owned(),
    };
    if let Err(err) = state.store.upsert_model(&new_model) {
        tracing::warn!("cannot register downloaded model {model_id}: {err}");
    }
    publish_progress(
        state,
        model_id,
        file_name,
        received,
        total,
        DownloadStatus::Complete,
    );
}

fn publish_progress(
    state: &AppState,
    model_id: &str,
    file_name: &str,
    received_bytes: u64,
    total_bytes: u64,
    status: DownloadStatus,
) {
    state
        .telemetry
        .publish(TelemetryEvent::model_download_progress(
            DownloadProgressData {
                model_id: model_id.to_string(),
                file_name: file_name.to_string(),
                received_bytes,
                total_bytes,
                status,
            },
        ));
}

/// Rejects anything that is not a plain `.gguf` file inside a well-formed
/// `owner/name` Hugging Face repository (documented `400` contract).
fn validate(repo_id: &str, file_name: &str) -> ApiResult<()> {
    if !file_name.to_ascii_lowercase().ends_with(".gguf") {
        return Err(ApiError::BadRequest("file_name must end with .gguf".into()));
    }
    if file_name.is_empty()
        || file_name.len() > 255
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
        || file_name.chars().any(char::is_control)
    {
        return Err(ApiError::BadRequest(
            "file_name must be a plain file name without path separators".into(),
        ));
    }
    if repo_id.is_empty() || repo_id.len() > 255 {
        return Err(ApiError::BadRequest("repo_id must not be empty".into()));
    }
    for segment in repo_id.split('/') {
        let acceptable = !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !acceptable {
            return Err(ApiError::BadRequest(
                "repo_id must look like an owner/name Hugging Face repository".into(),
            ));
        }
    }
    Ok(())
}

/// Model id used for store registration (`model-q4_k_m.gguf` → `model-q4_k_m`),
/// matching what the dashboard's "installed" check looks up.
fn model_id_from_file_name(file_name: &str) -> String {
    // `validate` guarantees the ASCII `.gguf` suffix, so `len - 5` is a char
    // boundary and at least zero.
    file_name
        .get(..file_name.len().saturating_sub(5))
        .unwrap_or(file_name)
        .to_string()
}

/// Best-effort quantisation tag from a file name
/// (`…-q4_k_m.gguf` → `Q4_K_M`, unknown → `unknown`).
fn quantization_from_file_name(file_name: &str) -> String {
    let bytes = file_name.as_bytes();
    for start in 0..bytes.len().saturating_sub(1) {
        if !matches!(bytes[start], b'q' | b'Q') || !bytes[start + 1].is_ascii_digit() {
            continue;
        }
        let mut end = start + 2;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        if end - start >= 3 {
            return file_name[start..end].to_ascii_uppercase();
        }
    }
    "unknown".into()
}

/// Builds the canonical Hugging Face resolve URL (segments are percent-encoded
/// by `url::PathSegmentsMut`).
fn resolve_url(repo_id: &str, file_name: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse("https://huggingface.co").map_err(|err| err.to_string())?;
    let mut segments = url
        .path_segments_mut()
        .map_err(|_| "repository URL cannot be built".to_string())?;
    segments.pop_if_empty();
    for segment in repo_id.split('/') {
        segments.push(segment);
    }
    segments.push("resolve").push("main").push(file_name);
    drop(segments);
    Ok(url.to_string())
}

async fn read_block_count_async(path: &Path) -> Option<u32> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || read_block_count(&path))
        .await
        .ok()
        .flatten()
}

/// Reads the transformer layer count (`*.block_count` metadata KV) from the
/// GGUF header at the start of `path`. `None` when the file is not GGUF or the
/// KV is not present within [`GGUF_SCAN_CAP`] bytes.
fn read_block_count(path: &Path) -> Option<u32> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut buffer = Vec::new();
    file.take(GGUF_SCAN_CAP).read_to_end(&mut buffer).ok()?;
    block_count_from_bytes(&buffer)
}

fn block_count_from_bytes(buffer: &[u8]) -> Option<u32> {
    if !buffer.starts_with(b"GGUF") {
        return None;
    }
    let mut parser = Parser {
        buffer,
        position: 4,
    };
    let version = parser.u32()?;
    if !(2..=3).contains(&version) {
        return None;
    }
    let _tensor_count = parser.u64()?;
    let kv_count = parser.u64()?;
    for _ in 0..kv_count {
        let key = parser.string()?;
        let value_type = parser.u32()?;
        if key.ends_with("block_count") {
            return match value_type {
                4 | 5 => parser.u32(),
                10 | 11 => u32::try_from(parser.u64()?).ok(),
                _ => None,
            };
        }
        parser.skip_value(value_type)?;
    }
    None
}

/// Sequential reader over a borrowed GGUF byte buffer.
struct Parser<'a> {
    buffer: &'a [u8],
    position: usize,
}

impl<'a> Parser<'a> {
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.position.checked_add(length)?;
        let bytes = self.buffer.get(self.position..end)?;
        self.position = end;
        Some(bytes)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn string(&mut self) -> Option<String> {
        let length = usize::try_from(self.u64()?).ok()?;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec()).ok()
    }

    /// Skips one typed GGUF metadata value. Arrays are traversed element-wise;
    /// nested arrays do not occur in practice and abort the scan.
    fn skip_value(&mut self, value_type: u32) -> Option<()> {
        match value_type {
            0 | 1 | 7 => self.take(1).map(|_| ()),
            2 | 3 => self.take(2).map(|_| ()),
            4..=6 => self.take(4).map(|_| ()),
            8 => self.string().map(|_| ()),
            9 => {
                let element_type = self.u32()?;
                if element_type == 9 {
                    return None;
                }
                let count = usize::try_from(self.u64()?).ok()?;
                for _ in 0..count {
                    self.skip_value(element_type)?;
                }
                Some(())
            }
            10..=12 => self.take(8).map(|_| ()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_gguf_and_traversal_names() {
        assert!(validate("owner/repo", "model.bin").is_err());
        assert!(validate("owner/repo", "../evil.gguf").is_err());
        assert!(validate("owner/repo", "sub/model.gguf").is_err());
        assert!(validate("owner/repo", "sub\\model.gguf").is_err());
        assert!(validate("", "model.gguf").is_err());
        assert!(validate("owner//repo", "model.gguf").is_err());
        assert!(validate("owner/repo/../x", "model.gguf").is_err());
        assert!(validate("owner/repo", "model.gguf").is_ok());
        assert!(validate("owner/repo-name.v2", "MODEL.GGUF").is_ok());
    }

    #[test]
    fn derives_model_id_and_quantization_from_file_name() {
        assert_eq!(
            model_id_from_file_name("tiny-model-q4_k_m.gguf"),
            "tiny-model-q4_k_m"
        );
        assert_eq!(
            quantization_from_file_name("qwen2.5-14b-instruct-q4_k_m.gguf"),
            "Q4_K_M"
        );
        assert_eq!(quantization_from_file_name("phi-3-mini-q8_0.gguf"), "Q8_0");
        assert_eq!(quantization_from_file_name("model.gguf"), "unknown");
    }

    #[test]
    fn builds_hugging_face_resolve_url() {
        let url =
            resolve_url("Qwen/Qwen2.5-14B-Instruct-GGUF", "qwen2.5-14b-q4_k_m.gguf").expect("url");
        assert_eq!(
            url,
            "https://huggingface.co/Qwen/Qwen2.5-14B-Instruct-GGUF/resolve/main/qwen2.5-14b-q4_k_m.gguf"
        );
        let encoded = resolve_url("owner/repo", "a b.gguf").expect("url");
        assert!(encoded.ends_with("/resolve/main/a%20b.gguf"));
    }

    #[test]
    fn parses_block_count_from_gguf_header() {
        assert_eq!(
            block_count_from_bytes(&StubFetcher::gguf_fixture()),
            Some(16)
        );
        assert_eq!(block_count_from_bytes(b"not a gguf file"), None);
        assert_eq!(block_count_from_bytes(b"GGUF"), None);
    }

    #[test]
    fn parses_block_count_behind_other_metadata_values() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"GGUF");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&3u64.to_le_bytes());

        // string KV: general.architecture
        let key = b"general.architecture";
        bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(&8u32.to_le_bytes());
        let value = b"llama";
        bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
        bytes.extend_from_slice(value);

        // string-array KV: general.names
        let key = b"general.names";
        bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(&9u32.to_le_bytes());
        bytes.extend_from_slice(&8u32.to_le_bytes()); // element type: string
        bytes.extend_from_slice(&2u64.to_le_bytes()); // count
        for name in [&b"a"[..], &b"bb"[..]] {
            bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
            bytes.extend_from_slice(name);
        }

        // i64 KV carrying the layer count
        let key = b"llama.block_count";
        bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(&11u32.to_le_bytes());
        bytes.extend_from_slice(&32i64.to_le_bytes());

        assert_eq!(block_count_from_bytes(&bytes), Some(32));
    }
}
