//! Worker health polling (spec 2026-10-22, D-E1/D-E2).
//!
//! The workers already measure themselves — the Android worker reads battery
//! capacity and a best-effort CPU temperature and answers `GetHealthStatus`
//! with them. The master used to never ask, and published `0.0` placeholders
//! instead. This module closes that loop: a background task asks every
//! discovered worker how it is doing, keeps the freshest snapshot per node, and
//! publishes it to the dashboard.
//!
//! The poll never runs on the generation hot path: `publish_metrics` fires per
//! streaming token, so it reads the cache instead of dialing anyone.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::api::AppState;
use crate::discovery::mdns::{DiscoveredNode, NodeRegistry};
use crate::rpc::engine::{dial_worker, WorkerTransportSource};
use crate::telemetry::{TelemetryBus, TelemetryEvent, WorkerHealth, WorkerHealthEventData};

/// How often the poller asks each worker for its health.
///
/// Battery and thermal move slowly; 5 s keeps the dashboard live without
/// churning the phone's radio (D-E1).
pub const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Per-node timeout for one health round trip (dial + RPC), so one dead node
/// cannot delay the rest of the cycle (D-E1, D-E8).
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

/// The freshest telemetry the master has: one snapshot per worker plus the
/// master's own RAM utilization.
///
/// A worker that fails to answer keeps its previous snapshot — it was really
/// measured, and `observed_at_epoch_ms` says how old it is. A worker that has
/// never answered has no entry at all, which reads as "unknown" rather than
/// zero (D-E2).
#[derive(Debug, Default)]
pub struct TelemetryCache {
    workers: Mutex<HashMap<String, WorkerHealth>>,
    master: Mutex<Option<(Option<f64>, u64)>>,
}

impl TelemetryCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a worker's fresh snapshot.
    pub fn record_worker(&self, health: WorkerHealth) {
        self.workers
            .lock()
            .expect("telemetry cache lock")
            .insert(health.id.clone(), health);
    }

    /// The last snapshot of one worker, if it has ever answered.
    pub fn worker(&self, node_id: &str) -> Option<WorkerHealth> {
        self.workers
            .lock()
            .expect("telemetry cache lock")
            .get(node_id)
            .cloned()
    }

    /// Every known worker snapshot, ordered by node id.
    pub fn workers(&self) -> Vec<WorkerHealth> {
        let mut workers: Vec<WorkerHealth> = self
            .workers
            .lock()
            .expect("telemetry cache lock")
            .values()
            .cloned()
            .collect();
        workers.sort_by(|a, b| a.id.cmp(&b.id));
        workers
    }

    /// The last measured temperature of every worker that reported one, in
    /// celsius (spec 2026-10-22, D-F1). A worker that cannot measure its
    /// temperature contributes nothing: unknown is never hot.
    pub fn temperatures(&self) -> HashMap<String, f64> {
        self.workers
            .lock()
            .expect("telemetry cache lock")
            .iter()
            .filter_map(|(id, health)| health.temp_celsius.map(|celsius| (id.clone(), celsius)))
            .collect()
    }

    /// Records the master's own RAM utilization (`None` when unreadable).
    pub fn record_master_ram(&self, utilization_pct: Option<f64>) {
        *self.master.lock().expect("telemetry cache lock") =
            Some((utilization_pct, now_epoch_ms()));
    }

    /// The master's own RAM utilization and when it was measured.
    pub fn master_ram(&self) -> Option<(Option<f64>, u64)> {
        *self.master.lock().expect("telemetry cache lock")
    }

    /// Drops the workers that are no longer in the registry (D-E8).
    pub fn retain_workers(&self, live: &HashSet<String>) {
        self.workers
            .lock()
            .expect("telemetry cache lock")
            .retain(|id, _| live.contains(id));
    }
}

/// One live memory reading for the host.
///
/// `used_bytes` is derived from `available_bytes`, not from `free_bytes`: the
/// operating system can reclaim cache, so free-only readings overstate what is
/// actually in use. The topology and the master utilization percentage share
/// this reading so they cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostMemoryUsage {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

/// Reads the master's own memory. A trait so tests can inject a reading
/// without depending on the machine they run on (D-E6).
pub trait HostMemorySource: Send + Sync {
    /// RAM utilization in percent, or `None` when it cannot be read.
    fn utilization_pct(&self) -> Option<f64>;

    /// Live total/used/available bytes, or `None` when it cannot be read.
    ///
    /// Defaults to unknown so existing fakes that only model utilization keep
    /// compiling; the topology then reports the master's RAM as `null` rather
    /// than inventing a number.
    fn usage(&self) -> Option<HostMemoryUsage> {
        None
    }
}

/// Reads the master's memory through `sysinfo` — the same crate and version
/// the worker crate already uses (D-E6).
pub struct SysinfoHostMemory;

impl HostMemorySource for SysinfoHostMemory {
    fn utilization_pct(&self) -> Option<f64> {
        self.usage()
            .map(|usage| (usage.used_bytes as f64 / usage.total_bytes as f64) * 100.0)
    }

    fn usage(&self) -> Option<HostMemoryUsage> {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        let total = system.total_memory();
        if total == 0 {
            return None;
        }
        let reported = system.available_memory();
        let available = if reported > 0 {
            reported
        } else {
            system.free_memory()
        }
        .min(total);
        Some(HostMemoryUsage {
            total_bytes: total,
            used_bytes: total - available,
            available_bytes: available,
        })
    }
}

/// Polls every discovered worker, forever (D-E1).
///
/// Spawned by the API server so every embedder of the core API gets live
/// telemetry without extra wiring.
///
/// The transport source is resolved **per cycle**, not once at spawn, so a
/// pairing that completes while the app is running is picked up on the next
/// poll — and the same resolution path the benchmark probe uses applies
/// (D-E4).
pub async fn worker_health_poll(state: AppState) {
    loop {
        let transports = state.transports();
        poll_once(
            &state.registry,
            transports.as_ref(),
            &state.telemetry,
            &state.telemetry_cache,
            state.host_memory.as_ref(),
        )
        .await;
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }
}

/// One poll cycle: ask every node, refresh the cache, publish the result.
///
/// Split from the loop so tests exercise the cycle without waiting (D-E1).
pub async fn poll_once(
    registry: &NodeRegistry,
    transports: &dyn WorkerTransportSource,
    telemetry: &TelemetryBus,
    cache: &TelemetryCache,
    host_memory: &dyn HostMemorySource,
) {
    let nodes = registry.list_nodes();
    let mut live = HashSet::with_capacity(nodes.len());

    for node in nodes {
        live.insert(node.node_id.clone());
        // An ADB-discovered phone whose worker app has not answered the
        // handshake has no dialable port yet. Its live capacity is still shown
        // in topology; there is nothing to poll until the app comes up.
        if node.port == 0 {
            continue;
        }
        match tokio::time::timeout(HEALTH_TIMEOUT, fetch_health(transports, &node)).await {
            Ok(Ok(health)) => cache.record_worker(health),
            Ok(Err(reason)) => {
                tracing::warn!("worker health poll failed for {}: {reason}", node.node_id);
            }
            Err(_) => {
                tracing::warn!(
                    "worker health poll timed out for {} after {HEALTH_TIMEOUT:?}",
                    node.node_id
                );
            }
        }
    }

    cache.retain_workers(&live);
    cache.record_master_ram(host_memory.utilization_pct());
    telemetry.publish(TelemetryEvent::worker_health(WorkerHealthEventData {
        master_ram_utilization_pct: cache.master_ram().and_then(|(value, _)| value),
        nodes: cache.workers(),
    }));
}

/// Asks one worker how it is doing.
async fn fetch_health(
    transports: &dyn WorkerTransportSource,
    node: &DiscoveredNode,
) -> Result<WorkerHealth, String> {
    let mut client = dial_worker(transports, node).await?;
    let response = client.health().await?;
    Ok(WorkerHealth {
        id: node.node_id.clone(),
        temp_celsius: known(response.cpu_temp_celsius),
        battery_pct: known(response.battery_percentage),
        free_ram_bytes: response.free_ram_bytes.max(0) as u64,
        observed_at_epoch_ms: now_epoch_ms(),
        supports_compute: response.supports_compute,
    })
}

/// A negative value is the worker's "I cannot measure this" sentinel — the
/// Android worker reports `-1f` when the thermal sysfs is unreadable, and the
/// Rust worker reports `-1.0` for the battery it does not have (D-E3).
fn known(value: f32) -> Option<f64> {
    if value.is_finite() && value >= 0.0 {
        Some(f64::from(value))
    } else {
        None
    }
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_negative_sentinel_is_unknown_not_zero() {
        assert_eq!(known(-1.0), None);
        assert_eq!(known(-0.5), None);
        assert_eq!(known(f32::NAN), None);
        assert_eq!(known(0.0), Some(0.0));
        assert_eq!(known(38.5), Some(38.5));
    }

    #[test]
    fn the_cache_keeps_the_last_snapshot_and_drops_departed_workers() {
        let cache = TelemetryCache::new();
        assert!(cache.worker("w1").is_none());

        cache.record_worker(WorkerHealth {
            id: "w1".into(),
            temp_celsius: Some(38.5),
            battery_pct: Some(92.0),
            free_ram_bytes: 1024,
            observed_at_epoch_ms: 1,
            supports_compute: true,
        });
        assert_eq!(cache.worker("w1").unwrap().temp_celsius, Some(38.5));

        let live: HashSet<String> = ["w2".to_string()].into_iter().collect();
        cache.retain_workers(&live);
        assert!(cache.worker("w1").is_none());
        assert!(cache.workers().is_empty());
    }

    #[test]
    fn the_master_ram_reading_is_unknown_until_it_is_recorded() {
        let cache = TelemetryCache::new();
        assert!(cache.master_ram().is_none());
        cache.record_master_ram(None);
        assert_eq!(cache.master_ram().unwrap().0, None);
        cache.record_master_ram(Some(42.0));
        assert_eq!(cache.master_ram().unwrap().0, Some(42.0));
    }
}
