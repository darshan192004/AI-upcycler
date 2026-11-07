//! HTTP surface for pushing a model to one phone (spec 2026-11-03-model-
//! provisioning, D-P5/D-P8/D-P9/D-P10).
//!
//! The transfer never rides on the HTTP request: a 1.9 GB model over Wi-Fi
//! takes minutes, so `POST` validates, claims the device, spawns the push and
//! answers `202`; `GET` reports how far it got. Progress lives in host memory
//! only — a restart reports `idle` rather than a stale transfer (D-P8).

use super::AppState;
use crate::discovery::mdns::DiscoveredNode;
use crate::provisioning::ModelSource;
use crate::rpc::engine::{dial_worker, WorkerTransportSource};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type ApiError = (StatusCode, String);

/// What a push to one device is doing, as `GET .../provision` reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProvisionState {
    Idle,
    Running { bytes_done: u64, total_bytes: u64 },
    Done { bytes: u64 },
    Failed { message: String },
}

/// Per-device push state, kept in host memory (spec D-P8).
///
/// Nothing here is persisted: the transfer it describes only exists while
/// this process runs, so a restart honestly reports `idle` instead of a
/// "running" push that will never move again.
#[derive(Debug, Default)]
pub struct ProvisioningRegistry {
    states: Mutex<HashMap<String, ProvisionState>>,
}

impl ProvisioningRegistry {
    /// The state for `id`. An unknown device is `Idle`, which is also what a
    /// fresh registry answers, so `GET` needs no special case.
    pub fn status(&self, id: &str) -> ProvisionState {
        self.states
            .lock()
            .expect("provisioning lock")
            .get(id)
            .cloned()
            .unwrap_or(ProvisionState::Idle)
    }

    /// Claims the device for a new push; `false` when one is already running,
    /// which is what makes a second `POST` a `409` (spec D-P8).
    pub fn try_begin(&self, id: &str, total_bytes: u64) -> bool {
        let mut states = self.states.lock().expect("provisioning lock");
        if matches!(states.get(id), Some(ProvisionState::Running { .. })) {
            return false;
        }
        states.insert(
            id.to_string(),
            ProvisionState::Running {
                bytes_done: 0,
                total_bytes,
            },
        );
        true
    }

    /// Records how many bytes the device holds so far.
    ///
    /// Ignored unless a push is running, so a late callback from a stream that
    /// has already finished cannot overwrite `done` or `failed`.
    pub fn progress(&self, id: &str, bytes_done: u64) {
        let mut states = self.states.lock().expect("provisioning lock");
        if let Some(ProvisionState::Running {
            bytes_done: slot,
            total_bytes,
        }) = states.get_mut(id)
        {
            // Never past the end: the resumed prefix plus what this session
            // sent is the file, and more would be a lie to the progress bar.
            *slot = bytes_done.min(*total_bytes);
        }
    }

    /// Ends the push: `Ok(bytes)` → `Done`, `Err(message)` → `Failed`.
    pub fn finish(&self, id: &str, result: Result<u64, String>) {
        let state = match result {
            Ok(bytes) => ProvisionState::Done { bytes },
            Err(message) => ProvisionState::Failed { message },
        };
        self.states
            .lock()
            .expect("provisioning lock")
            .insert(id.to_string(), state);
    }
}

#[derive(Debug, Deserialize)]
pub struct ProvisionRequest {
    /// A model id from `/v1/models`, never a path (spec D-P10).
    pub model_id: String,
}

/// `POST /api/devices/:id/provision` — start a push (spec D-P5: always an
/// explicit user action), answering `202` with the starting state.
pub async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ProvisionRequest>,
) -> Result<(StatusCode, Json<ProvisionState>), ApiError> {
    // Everything is resolved from the request id, never taken from the body:
    // the node from the registry, the file from the store row (D-P10).
    let node = resolve_node(&state, &id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("no cluster node is registered for device {id}"),
        )
    })?;

    let row = state
        .store
        .get_model(&request.model_id)
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("model lookup failed: {error}"),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("unknown model {}", request.model_id),
            )
        })?;

    // The whole point of the slice is the mTLS data plane: a plaintext 1.9 GB
    // push to an unauthenticated peer is not something to make convenient
    // (spec D-P9). Checked before the file is touched at all.
    let canonical = state.registry.canonical_id(&node.node_id);
    if !state.security.enrollments.is_enrolled(&canonical)
        && !state.security.enrollments.is_enrolled(&node.node_id)
    {
        return Err((
            StatusCode::CONFLICT,
            format!("device {id} is not enrolled; pair it before provisioning"),
        ));
    }

    let source = ModelSource::open(&row.local_file_path).map_err(|error| {
        (
            StatusCode::CONFLICT,
            format!("{} is not available to push: {error}", row.local_file_path),
        )
    })?;

    if !state.provisioning.try_begin(&id, source.size_bytes()) {
        return Err((
            StatusCode::CONFLICT,
            format!("a model push to {id} is already running"),
        ));
    }

    let total = source.size_bytes();
    let registry = Arc::clone(&state.provisioning);
    let key = id.clone();
    let model_id = request.model_id.clone();
    let transports = state.transports();
    tokio::spawn(async move {
        let result = push_to_worker(key.clone(), model_id, source, node, transports, {
            Arc::clone(&registry)
        })
        .await;
        registry.finish(&key, result);
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(ProvisionState::Running {
            bytes_done: 0,
            total_bytes: total,
        }),
    ))
}

/// `GET /api/devices/:id/provision` — the current state (spec D-P8).
pub async fn status(State(state): State<AppState>, Path(id): Path<String>) -> Json<ProvisionState> {
    Json(state.provisioning.status(&id))
}

/// Runs one full push and reports how far it got.
///
/// Returns the bytes the worker installed, which is what `Done` reports — not
/// what this session happened to send, in case the worker already held a
/// resumed prefix.
async fn push_to_worker(
    key: String,
    model_id: String,
    source: ModelSource,
    node: DiscoveredNode,
    transports: Arc<dyn WorkerTransportSource>,
    registry: Arc<ProvisioningRegistry>,
) -> Result<u64, String> {
    let mut client = dial_worker(&*transports, &node)
        .await
        .map_err(|error| format!("provision {model_id}: {error}"))?;
    let reply = client
        .provision_model_with_progress(&source, &model_id, move |bytes| {
            registry.progress(&key, bytes);
        })
        .await?;
    if !reply.engine_started {
        // Not a failure: the bytes are on the phone and the worker reopens the
        // file a moment later (spec D-P7).
        tracing::info!(
            "provisioning: {model_id} verified on {}; the engine reopens it on its next start",
            node.device_name
        );
    }
    Ok(reply.size_bytes.max(0) as u64)
}

/// The registry node a device id refers to.
///
/// A device card is keyed three ways, so all three resolve: the mDNS node id,
/// the bare ADB serial (with or without its `adb:` alias), and the `ip:port`
/// form discovery uses until `adb` reports a serial.
fn resolve_node(state: &AppState, id: &str) -> Option<DiscoveredNode> {
    let nodes = state.registry.list_nodes();

    let adb_id = format!("adb:{id}");
    let mut candidates = vec![
        id.to_string(),
        state.registry.canonical_id(id),
        adb_id.clone(),
        state.registry.canonical_id(&adb_id),
    ];
    candidates.dedup();
    if let Some(node) = nodes.iter().find(|node| candidates.contains(&node.node_id)) {
        return Some(node.clone());
    }

    // `ip:port`: the key discovery uses before a serial is known.
    let (host, port) = id.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    nodes
        .into_iter()
        .find(|node| node.ip_address == host && node.port == port)
}
