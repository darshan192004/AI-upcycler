//! Real-time telemetry fan-out.
//!
//! A single [`TelemetryBus`] (a `tokio` broadcast channel) carries cluster
//! events to every connected WebSocket subscriber. The registry publishes
//! node lifecycle events into it, and the API publishes per-generation metrics.

use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

/// Buffered events retained for slow subscribers before they are dropped.
pub const DEFAULT_CHANNEL_CAPACITY: usize = 256;

/// Snapshot of a node used by connect/disconnect events.
#[derive(Debug, Clone, Serialize)]
pub struct NodeEventData {
    pub node_id: String,
    pub device_name: String,
    pub connection_type: String,
    pub total_ram_bytes: u64,
    pub free_ram_bytes: u64,
}

/// Per-worker telemetry carried inside a [`GenerationMetrics`] event.
///
/// Thermal/battery values are `None` when the worker cannot measure them —
/// the master never fills in a placeholder zero (spec 2026-10-22, D-E3).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkerTelemetry {
    pub id: String,
    pub temp_celsius: Option<f64>,
    pub battery_pct: Option<f64>,
}

/// One worker's latest health snapshot, as published to the dashboard.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkerHealth {
    pub id: String,
    /// CPU temperature in celsius; `None` when the worker cannot measure it.
    pub temp_celsius: Option<f64>,
    /// Battery charge in percent; `None` when the worker cannot measure it.
    pub battery_pct: Option<f64>,
    pub free_ram_bytes: u64,
    /// When the snapshot was taken (Unix epoch milliseconds).
    pub observed_at_epoch_ms: u64,
    /// Whether the worker can execute layer RPCs, i.e. a model engine is open.
    /// A metadata-only worker still answers health but must never be planned
    /// over (its `LoadLayers` would fail with `UNIMPLEMENTED`).
    pub supports_compute: bool,
}

/// The freshest cluster telemetry, published after every poll cycle.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkerHealthEventData {
    /// The master's own RAM utilization; `None` when it could not be read.
    pub master_ram_utilization_pct: Option<f64>,
    /// One entry per worker that has ever answered.
    pub nodes: Vec<WorkerHealth>,
}

/// Inference throughput/utilisation event emitted while tokens stream.
#[derive(Debug, Clone, Serialize)]
pub struct GenerationMetrics {
    pub tokens_per_second: f64,
    pub current_token: u64,
    /// The master's own RAM utilization; `None` when it could not be read.
    pub master_ram_utilization_pct: Option<f64>,
    pub worker_nodes: Vec<WorkerTelemetry>,
}

/// Lifecycle of a GGUF download started via `POST /api/models/download`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Running,
    Complete,
    Error,
}

/// Progress carried inside a [`ModelDownloadProgress`] event.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgressData {
    pub model_id: String,
    pub file_name: String,
    pub received_bytes: u64,
    pub total_bytes: u64,
    pub status: DownloadStatus,
}

/// A telemetry message, serialised for the wire as
/// `{"event": "...", "timestamp": <secs>, "data": { ... }}` (see `docs/api.md`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TelemetryEvent {
    NodeConnected {
        timestamp: u64,
        data: NodeEventData,
    },
    NodeDisconnected {
        timestamp: u64,
        data: NodeEventData,
    },
    GenerationMetrics {
        timestamp: u64,
        data: GenerationMetrics,
    },
    WorkerHealth {
        timestamp: u64,
        data: WorkerHealthEventData,
    },
    ModelDownloadProgress {
        timestamp: u64,
        data: DownloadProgressData,
    },
}

impl TelemetryEvent {
    pub fn node_connected(data: NodeEventData) -> Self {
        Self::NodeConnected {
            timestamp: now_epoch_secs(),
            data,
        }
    }

    pub fn node_disconnected(data: NodeEventData) -> Self {
        Self::NodeDisconnected {
            timestamp: now_epoch_secs(),
            data,
        }
    }

    pub fn generation_metrics(data: GenerationMetrics) -> Self {
        Self::GenerationMetrics {
            timestamp: now_epoch_secs(),
            data,
        }
    }

    /// The freshest worker health, published after every poll cycle.
    pub fn worker_health(data: WorkerHealthEventData) -> Self {
        Self::WorkerHealth {
            timestamp: now_epoch_secs(),
            data,
        }
    }

    pub fn model_download_progress(data: DownloadProgressData) -> Self {
        Self::ModelDownloadProgress {
            timestamp: now_epoch_secs(),
            data,
        }
    }
}

/// Broadcast fan-out handle. Cheaply cloneable; cloning keeps the same channel.
#[derive(Clone)]
pub struct TelemetryBus {
    sender: broadcast::Sender<TelemetryEvent>,
}

impl TelemetryBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _receiver) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Publishes an event; returns the number of receivers that received it
    /// (`0` when nobody is subscribed — never an error).
    pub fn publish(&self, event: TelemetryEvent) -> usize {
        self.sender.send(event).unwrap_or(0)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TelemetryEvent> {
        self.sender.subscribe()
    }

    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for TelemetryBus {
    fn default() -> Self {
        Self::new(DEFAULT_CHANNEL_CAPACITY)
    }
}

pub fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NodeEventData {
        NodeEventData {
            node_id: "n1".into(),
            device_name: "Phone".into(),
            connection_type: "usb_adb".into(),
            total_ram_bytes: 1024,
            free_ram_bytes: 512,
        }
    }

    #[test]
    fn serialises_with_event_tag_and_data() {
        let event = TelemetryEvent::node_connected(sample());
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["event"], "node_connected");
        assert_eq!(value["data"]["node_id"], "n1");
        assert!(value["timestamp"].as_u64().is_some());
    }

    #[test]
    fn broadcasts_to_subscribers() {
        let bus = TelemetryBus::new(4);
        let mut rx = bus.subscribe();
        assert_eq!(bus.receiver_count(), 1);
        assert_eq!(bus.publish(TelemetryEvent::node_connected(sample())), 1);

        let received = rx.try_recv().expect("event delivered");
        assert!(matches!(received, TelemetryEvent::NodeConnected { .. }));
    }

    #[test]
    fn publish_without_subscribers_is_a_noop() {
        let bus = TelemetryBus::default();
        assert_eq!(bus.publish(TelemetryEvent::node_connected(sample())), 0);
    }

    #[test]
    fn serialises_download_progress_with_tagged_status() {
        let event = TelemetryEvent::model_download_progress(DownloadProgressData {
            model_id: "tiny-model".into(),
            file_name: "tiny-model-q4_k_m.gguf".into(),
            received_bytes: 512,
            total_bytes: 1024,
            status: DownloadStatus::Running,
        });
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["event"], "model_download_progress");
        assert_eq!(value["data"]["status"], "running");
        assert_eq!(value["data"]["received_bytes"], 512);

        let done = TelemetryEvent::model_download_progress(DownloadProgressData {
            model_id: "tiny-model".into(),
            file_name: "tiny-model-q4_k_m.gguf".into(),
            received_bytes: 1024,
            total_bytes: 1024,
            status: DownloadStatus::Complete,
        });
        assert_eq!(
            serde_json::to_value(&done).unwrap()["data"]["status"],
            "complete"
        );
    }
}
