//! Cluster management endpoints: topology view and hardware benchmarking.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::api::{ApiResult, AppState, HostInfo};
use crate::discovery::mdns::DiscoveredNode;
use crate::rpc::client::WorkerRpcClient;
use crate::rpc::engine::WorkerTransportSource;
use crate::scheduler::planner::{LayerScheduler, ModelSpec, ThermalHints};
use crate::store::{NewBenchmark, NewNode};
use crate::telemetry::WorkerHealth;
use crate::worker_health::HostMemoryUsage;

/// How the master sees a node in the topology graph.
#[derive(Debug, Serialize)]
pub struct Topology {
    pub cluster_id: String,
    pub status: &'static str,
    pub active_model: Option<String>,
    pub total_pooled_ram_mb: u64,
    /// Sum of the nodes' live used memory. Unknown readings are omitted, so
    /// this is a measured total, not an estimate padded with zeros.
    pub used_pooled_ram_mb: u64,
    /// Sum of the nodes' live available memory, with the same omission rule.
    pub available_pooled_ram_mb: u64,
    pub nodes: Vec<TopologyNode>,
    pub storage_mmap: StorageMmap,
    pub discovery: DiscoveryStatus,
    /// The thermal state the layer plan was computed against: one entry per
    /// worker **in the plan** that has a measured temperature (spec 2026-10-22,
    /// D-F4). A discovered-but-unpaired worker is not planned over, so it has
    /// no entry here even though its temperature is still measured.
    pub thermal: Vec<TopologyThermal>,
    /// Whether the hot-worker cap had to be relaxed to place the model
    /// (spec D-F3): the cluster was hot enough that resting was not an option.
    pub thermal_relaxed: bool,
}

/// Whether the mDNS browser behind Wi-Fi discovery is actually running.
#[derive(Debug, Serialize)]
pub struct DiscoveryStatus {
    pub mdns: &'static str,
}

/// The thermal state one node was planned against (spec 2026-10-22, D-F4).
#[derive(Debug, Serialize)]
pub struct TopologyThermal {
    pub node_id: String,
    /// The last measured CPU temperature, in celsius.
    pub temp_celsius: f64,
    /// Whether this node's share of the model was capped because it is hot.
    pub capped: bool,
}

#[derive(Debug, Serialize)]
pub struct TopologyNode {
    pub node_id: String,
    pub role: &'static str,
    pub device_name: String,
    pub connection: String,
    pub ram_total_mb: u64,
    /// Live used memory, or `null` when the reading is unavailable.
    pub ram_used_mb: Option<u64>,
    /// Live available memory, or `null` when the reading is unavailable.
    pub ram_available_mb: Option<u64>,
    /// What the scheduler planned to place here. Kept separate from live
    /// usage: the master coordinates and holds no layers, so its assignment
    /// is legitimately zero even while it is using memory.
    pub ram_assigned_mb: u64,
    /// `master`, `usb_detected`, `discovered`, `paired`, or `ready`.
    pub worker_state: &'static str,
    pub assigned_layers: Vec<[usize; 2]>,
    pub ping_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct StorageMmap {
    pub enabled: bool,
    pub path: String,
    pub mapped_layers_count: usize,
}

pub async fn topology(State(state): State<AppState>) -> ApiResult<Json<Topology>> {
    let mut workers = state.registry.list_nodes();
    workers.sort_by(|a, b| a.node_id.cmp(&b.node_id));

    let master = master_node(&state.host);
    // The layer assignments must be the plan a real request would use, so they
    // come from the same helper as the serving path: paired, compute-capable
    // workers only, in scheduler order (spec 2026-10-22, D-F4). The master
    // tokenizes and samples but holds no layers, an unpaired phone cannot be
    // dialed, and a paired phone without an open engine cannot execute a layer,
    // so none is planned over - they are still listed, with their real capacity.
    let plan_nodes = crate::api::distributed::serving_plan_nodes(
        &state.registry,
        &state.security.enrollments,
        &state.telemetry_cache,
    );

    let model = state.store.list_models()?.into_iter().next();

    let mut assignments: HashMap<String, Vec<[usize; 2]>> = HashMap::new();
    let mut ram_assigned: HashMap<String, u64> = HashMap::new();
    let mut active_model = None;
    let mut thermal: Vec<TopologyThermal> = Vec::new();
    let mut thermal_relaxed = false;
    if let Some(model) = &model {
        // The model's name is a fact about the library, not about the split: a
        // host with nothing paired still serves this model locally, so the
        // dashboard must not say "None loaded" merely because the cluster plan
        // is empty.
        active_model = Some(model.model_id.clone());
        if model.total_layers > 0 && model.weight_size_bytes > 0 {
            let spec = ModelSpec {
                model_id: model.model_id.clone(),
                total_layers: model.total_layers as usize,
                bytes_per_layer: model.weight_size_bytes as u64 / model.total_layers as u64,
                activation_bytes: 0,
                quantization: model.quantization_format.clone(),
            };
            let plan = LayerScheduler::compute_plan(
                &spec,
                &plan_nodes,
                &ThermalHints::from_temperatures(state.telemetry_cache.temperatures()),
            );
            thermal = plan
                .thermal
                .iter()
                .map(|node| TopologyThermal {
                    node_id: node.node_id.clone(),
                    temp_celsius: node.temp_celsius,
                    capped: node.capped,
                })
                .collect();
            thermal_relaxed = plan.thermal_relaxed;
            for assignment in &plan.assignments {
                assignments
                    .entry(assignment.node_id.clone())
                    .or_default()
                    .push([assignment.start_layer, assignment.end_layer]);
                *ram_assigned.entry(assignment.node_id.clone()).or_default() +=
                    assignment.total_ram_used_bytes;
            }
        }
    }

    let host_usage = state.host_memory.usage();
    let mut nodes = Vec::with_capacity(workers.len() + 1);
    nodes.push(build_topology_node(
        &master,
        "master",
        &assignments,
        &ram_assigned,
        host_usage,
        "master",
    ));
    for worker in &workers {
        let health = state.telemetry_cache.worker(&worker.node_id);
        let enrolled = state.security.enrollments.is_enrolled(&worker.node_id);
        nodes.push(build_topology_node(
            worker,
            "worker",
            &assignments,
            &ram_assigned,
            worker_live_memory(worker, health.as_ref()),
            worker_state(worker, enrolled, health.as_ref()),
        ));
    }

    let total_pooled_ram_mb = nodes.iter().map(|n| n.ram_total_mb).sum();
    let used_pooled_ram_mb = nodes.iter().filter_map(|n| n.ram_used_mb).sum();
    let available_pooled_ram_mb = nodes.iter().filter_map(|n| n.ram_available_mb).sum();

    Ok(Json(Topology {
        cluster_id: state.cluster_id.clone(),
        status: if active_model.is_some() {
            "active"
        } else {
            "idle"
        },
        active_model,
        total_pooled_ram_mb,
        used_pooled_ram_mb,
        available_pooled_ram_mb,
        nodes,
        storage_mmap: StorageMmap {
            enabled: false,
            path: String::new(),
            mapped_layers_count: 0,
        },
        discovery: DiscoveryStatus {
            mdns: if state.registry.mdns_active() {
                "active"
            } else {
                "unavailable"
            },
        },
        thermal,
        thermal_relaxed,
    }))
}

#[derive(Debug, Serialize)]
pub struct BenchmarkResponse {
    pub status: &'static str,
    pub nodes_tested: u32,
    /// Nodes that could not be measured — dial failures, timeouts, and workers
    /// that predate the probe RPC (gRPC `UNIMPLEMENTED`). Counted, never faked
    /// (spec 2026-10-21, D-C6).
    pub nodes_failed: u32,
    pub rebalancing_triggered: bool,
    pub execution_time_ms: u64,
}

/// Bounds a single node's probe. The worker's own probe is ~0.5 s by
/// construction (D-C2); 2 s leaves room for a slow dial plus handshake without
/// letting one unreachable node stall the endpoint (D-C6).
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

pub async fn benchmark(State(state): State<AppState>) -> ApiResult<Json<BenchmarkResponse>> {
    let started = Instant::now();
    // One resolution path for every dial: the enrollment-aware rule, or the
    // test override (spec 2026-10-22, D-E4).
    let benchmarker = ProbeBenchmarker::with_transports(state.transports());
    let mut nodes_tested = 0u32;
    let mut nodes_failed = 0u32;

    for node in state.registry.list_nodes() {
        state.store.upsert_node(&NewNode {
            node_id: node.node_id.clone(),
            device_name: node.device_name.clone(),
            mac_address: None,
            connection_type: connection_label(&node.connection_type),
            total_ram_mb: (node.total_ram_bytes / (1024 * 1024)) as i64,
            cpu_cores: 0,
            has_npu: false,
        })?;

        match tokio::time::timeout(PROBE_TIMEOUT, benchmarker.benchmark(&node)).await {
            Ok(Ok(result)) => {
                state.store.record_benchmark(&NewBenchmark {
                    node_id: node.node_id.clone(),
                    memory_bandwidth_mbs: result.memory_bandwidth_mbs,
                    network_ping_ms: result.network_ping_ms,
                    throughput_flops: result.throughput_flops,
                })?;
                nodes_tested += 1;
            }
            Ok(Err(reason)) => {
                tracing::warn!("benchmark probe failed for node {}: {reason}", node.node_id);
                nodes_failed += 1;
            }
            Err(_) => {
                tracing::warn!(
                    "benchmark probe timed out for node {} after {:?}",
                    node.node_id,
                    PROBE_TIMEOUT
                );
                nodes_failed += 1;
            }
        }
    }

    Ok(Json(BenchmarkResponse {
        status: benchmark_status(nodes_tested, nodes_failed),
        nodes_tested,
        nodes_failed,
        rebalancing_triggered: nodes_tested > 0,
        execution_time_ms: started.elapsed().as_millis() as u64,
    }))
}

/// Reports what actually happened, so an operator can tell a real measurement
/// from a silent no-op (D-C6): `no_nodes` when nothing was probed, `partial`
/// when some probes failed, `success` only when every probed node answered.
fn benchmark_status(nodes_tested: u32, nodes_failed: u32) -> &'static str {
    match (nodes_tested, nodes_failed) {
        (0, 0) => "no_nodes",
        (_, 0) => "success",
        _ => "partial",
    }
}

/// Result of running a node through the hardware benchmarker.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeBenchmark {
    pub memory_bandwidth_mbs: f64,
    pub network_ping_ms: f64,
    pub throughput_flops: f64,
}

/// Measures a node's usable compute/bandwidth characteristics.
///
/// Async because the real probe is an RPC (D-C5). Native async-fn-in-trait, not
/// the `async-trait` crate: the workspace has no such dependency and this trait
/// is never used as `dyn`, so monomorphization at each call site is enough.
#[allow(async_fn_in_trait)]
pub trait HardwareBenchmarker: Send + Sync {
    async fn benchmark(&self, node: &DiscoveredNode) -> Result<NodeBenchmark, String>;
}

/// Probes each node over the transport it actually serves on (D-C3).
pub struct ProbeBenchmarker {
    transports: Arc<dyn WorkerTransportSource>,
}

impl ProbeBenchmarker {
    /// Dials every node with the given transport source. Production resolves
    /// it through [`crate::api::AppState::transports`] — the enrollment-aware
    /// rule (mTLS once paired, plaintext until then, D-C3); tests inject a
    /// plaintext loopback source.
    pub fn with_transports(transports: Arc<dyn WorkerTransportSource>) -> Self {
        Self { transports }
    }

    async fn connect(&self, node: &DiscoveredNode) -> Result<WorkerRpcClient, String> {
        crate::rpc::engine::dial_worker(self.transports.as_ref(), node).await
    }
}

impl HardwareBenchmarker for ProbeBenchmarker {
    async fn benchmark(&self, node: &DiscoveredNode) -> Result<NodeBenchmark, String> {
        let mut client = self.connect(node).await?;
        let measured = client.benchmark().await?;
        if !measured.memory_bandwidth_mbs.is_finite() || !measured.throughput_flops.is_finite() {
            return Err(format!(
                "worker {} returned a non-finite measurement",
                node.node_id
            ));
        }
        Ok(NodeBenchmark {
            memory_bandwidth_mbs: measured.memory_bandwidth_mbs,
            // Discovery latency is a real measured round-trip; the probe RPC's
            // own round-trip is compute-dominated and would mislead (D-C4).
            network_ping_ms: f64::from(node.latency_ms),
            throughput_flops: measured.throughput_flops,
        })
    }
}

/// Deterministic placeholder benchmarker, **test-only** (D-C7).
///
/// Values are derived from a hash of the node id so results are stable across
/// runs, but they do not reflect real hardware. Nothing in production may use
/// this: the handler wires [`ProbeBenchmarker`]. Kept for tests and fixtures.
#[cfg(test)]
pub struct StubBenchmarker;

#[cfg(test)]
impl HardwareBenchmarker for StubBenchmarker {
    async fn benchmark(&self, node: &DiscoveredNode) -> Result<NodeBenchmark, String> {
        let hash = fnv1a(node.node_id.as_bytes());
        Ok(NodeBenchmark {
            memory_bandwidth_mbs: 400.0 + (hash % 600) as f64,
            network_ping_ms: node.latency_ms as f64,
            throughput_flops: 5.0 + (hash % 50) as f64,
        })
    }
}

#[cfg(test)]
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn master_node(host: &HostInfo) -> DiscoveredNode {
    let ram_bytes = host.total_ram_mb * 1024 * 1024;
    DiscoveredNode {
        node_id: host.node_id.clone(),
        device_name: host.device_name.clone(),
        ip_address: "127.0.0.1".into(),
        port: 0,
        total_ram_bytes: ram_bytes,
        free_ram_bytes: ram_bytes,
        connection_type: "native".into(),
        latency_ms: 0.0,
        last_seen_epoch_ms: 0,
    }
}

fn build_topology_node(
    node: &DiscoveredNode,
    role: &'static str,
    assignments: &HashMap<String, Vec<[usize; 2]>>,
    ram_assigned: &HashMap<String, u64>,
    live: Option<HostMemoryUsage>,
    worker_state: &'static str,
) -> TopologyNode {
    let total_bytes = live
        .map(|usage| usage.total_bytes)
        .unwrap_or(node.total_ram_bytes);
    TopologyNode {
        node_id: node.node_id.clone(),
        role,
        device_name: node.device_name.clone(),
        connection: connection_label(&node.connection_type),
        ram_total_mb: total_bytes / (1024 * 1024),
        ram_used_mb: live.map(|usage| usage.used_bytes / (1024 * 1024)),
        ram_available_mb: live.map(|usage| usage.available_bytes / (1024 * 1024)),
        ram_assigned_mb: ram_assigned.get(&node.node_id).copied().unwrap_or(0) / (1024 * 1024),
        worker_state,
        assigned_layers: assignments.get(&node.node_id).cloned().unwrap_or_default(),
        ping_ms: f64::from(node.latency_ms),
    }
}

/// Live memory for a worker: total comes from the discovered device, available
/// from the freshest health reading (or the device's own advertisement before
/// the first health answer). Unknown total stays unknown rather than zero.
fn worker_live_memory(
    node: &DiscoveredNode,
    health: Option<&WorkerHealth>,
) -> Option<HostMemoryUsage> {
    if node.total_ram_bytes == 0 {
        return None;
    }
    let available = health
        .map(|health| health.free_ram_bytes)
        .unwrap_or(node.free_ram_bytes)
        .min(node.total_ram_bytes);
    Some(HostMemoryUsage {
        total_bytes: node.total_ram_bytes,
        used_bytes: node.total_ram_bytes - available,
        available_bytes: available,
    })
}

/// Pairing proves identity; health proves an engine is open. The dashboard
/// distinguishes a phone that is merely on USB from one that is paired and
/// ready to receive layers.
fn worker_state(
    node: &DiscoveredNode,
    enrolled: bool,
    health: Option<&WorkerHealth>,
) -> &'static str {
    if enrolled {
        if health
            .map(|health| health.supports_compute)
            .unwrap_or(false)
        {
            "ready"
        } else {
            "paired"
        }
    } else if node.connection_type == "USB_ADB" {
        "usb_detected"
    } else {
        "discovered"
    }
}

fn connection_label(connection_type: &str) -> String {
    match connection_type {
        "USB_ADB" => "usb_adb".into(),
        "WIFI_MDNS" => "wifi_5g".into(),
        other => other.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::backend::StubBackend;
    use crate::discovery::mdns::NodeRegistry;
    use crate::store::{NewModel, Store};
    use crate::telemetry::TelemetryBus;
    use std::sync::Arc;

    fn node(id: &str, ram_bytes: u64) -> DiscoveredNode {
        DiscoveredNode {
            node_id: id.into(),
            device_name: format!("device-{id}"),
            ip_address: "10.0.0.5".into(),
            port: 50052,
            total_ram_bytes: ram_bytes,
            free_ram_bytes: ram_bytes,
            connection_type: "USB_ADB".into(),
            latency_ms: 1.5,
            last_seen_epoch_ms: 0,
        }
    }

    const GIB: u64 = 1024 * 1024 * 1024;

    struct FakeHostMemory {
        usage: Option<HostMemoryUsage>,
    }

    impl crate::worker_health::HostMemorySource for FakeHostMemory {
        fn utilization_pct(&self) -> Option<f64> {
            self.usage
                .map(|usage| (usage.used_bytes as f64 / usage.total_bytes as f64) * 100.0)
        }

        fn usage(&self) -> Option<HostMemoryUsage> {
            self.usage
        }
    }

    fn state(registry: Arc<NodeRegistry>, store: Arc<Store>) -> AppState {
        AppState::new(
            "cluster-1".into(),
            registry,
            store,
            Arc::new(StubBackend::new()),
            TelemetryBus::new(8),
            HostInfo {
                node_id: "master".into(),
                device_name: "Master".into(),
                total_ram_mb: 2048,
                cpu_cores: 4,
            },
        )
        .with_host_memory(Arc::new(FakeHostMemory {
            usage: Some(HostMemoryUsage {
                total_bytes: 2 * GIB,
                used_bytes: GIB,
                available_bytes: GIB,
            }),
        }))
    }

    async fn topology_of(state: AppState) -> Topology {
        let Json(topology) = topology(State(state)).await.unwrap();
        topology
    }

    #[tokio::test]
    async fn topology_lists_master_and_workers() {
        let registry = Arc::new(NodeRegistry::new());
        registry.register_or_update(node("w1", 4 * 1024 * 1024 * 1024));
        let store = Arc::new(Store::open_in_memory().unwrap());
        let state = state(registry, store);

        let topology = topology_of(state).await;

        assert_eq!(topology.cluster_id, "cluster-1");
        assert_eq!(topology.nodes.len(), 2);
        assert_eq!(topology.nodes[0].role, "master");
        assert_eq!(topology.nodes[0].worker_state, "master");
        assert_eq!(topology.nodes[0].ram_used_mb, Some(1024));
        assert_eq!(topology.nodes[0].ram_available_mb, Some(1024));
        assert_eq!(topology.nodes[0].ram_assigned_mb, 0);
        assert_eq!(topology.nodes[1].role, "worker");
        assert_eq!(topology.nodes[1].connection, "usb_adb");
        assert_eq!(topology.nodes[1].worker_state, "usb_detected");
        assert_eq!(topology.nodes[1].ram_used_mb, Some(0));
        assert_eq!(topology.nodes[1].ram_available_mb, Some(4096));
        assert_eq!(topology.status, "idle");
        assert_eq!(topology.total_pooled_ram_mb, 2048 + 4096);
        assert_eq!(topology.used_pooled_ram_mb, 1024);
        assert_eq!(topology.available_pooled_ram_mb, 1024 + 4096);
        assert_eq!(topology.discovery.mdns, "unavailable");
    }

    #[tokio::test]
    async fn topology_uses_live_memory_not_scheduler_assignment() {
        let registry = Arc::new(NodeRegistry::new());
        let mut worker = node("w1", 4 * GIB);
        worker.free_ram_bytes = GIB;
        registry.register_or_update(worker);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let state = state(registry, store);

        let topology = topology_of(state).await;
        let master = topology.nodes.iter().find(|n| n.role == "master").unwrap();
        let worker = topology.nodes.iter().find(|n| n.role == "worker").unwrap();

        assert_eq!(master.ram_used_mb, Some(1024));
        assert_eq!(worker.ram_used_mb, Some(3 * 1024));
        assert_eq!(worker.ram_available_mb, Some(1024));
        assert_eq!(
            worker.ram_assigned_mb, 0,
            "no model plan means no assignment"
        );
        assert_eq!(topology.used_pooled_ram_mb, 1024 + 3 * 1024);
        assert_eq!(topology.available_pooled_ram_mb, 1024 + 1024);
    }

    #[tokio::test]
    async fn topology_marks_an_enrolled_worker_without_an_engine_as_paired() {
        let registry = Arc::new(NodeRegistry::new());
        registry.register_or_update(node("w1", 4 * GIB));
        let store = Arc::new(Store::open_in_memory().unwrap());
        let state = state(registry, store);
        state.security.enrollments.enroll("w1", "w1", vec![]);

        let topology = topology_of(state).await;
        let worker = topology.nodes.iter().find(|n| n.role == "worker").unwrap();

        assert_eq!(worker.worker_state, "paired");
    }

    #[tokio::test]
    async fn topology_marks_a_usb_phone_before_pairing() {
        let registry = Arc::new(NodeRegistry::new());
        let mut phone = node("adb:16284c49", 5 * GIB);
        phone.device_name = "Redmi_Note_9_Pro_Max".into();
        phone.free_ram_bytes = 2 * GIB;
        phone.port = 0;
        registry.register_or_update(phone);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let state = state(registry, store);

        let topology = topology_of(state).await;
        let phone = topology
            .nodes
            .iter()
            .find(|n| n.node_id == "adb:16284c49")
            .expect("USB phone listed");

        assert_eq!(phone.worker_state, "usb_detected");
        assert_eq!(phone.ram_total_mb, 5 * 1024);
        assert_eq!(phone.ram_used_mb, Some(3 * 1024));
        assert_eq!(phone.ram_available_mb, Some(2 * 1024));
        assert_eq!(phone.ram_assigned_mb, 0);
    }

    #[tokio::test]
    async fn topology_marks_a_wifi_node_as_discovered() {
        let registry = Arc::new(NodeRegistry::new());
        let mut wifi = node("wifi-1", 4 * GIB);
        wifi.connection_type = "WIFI_MDNS".into();
        registry.register_or_update(wifi);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let state = state(registry, store);

        let topology = topology_of(state).await;
        let wifi = topology
            .nodes
            .iter()
            .find(|n| n.node_id == "wifi-1")
            .unwrap();
        assert_eq!(wifi.worker_state, "discovered");
    }

    #[tokio::test]
    async fn topology_reports_active_mdns_discovery() {
        let registry = Arc::new(NodeRegistry::new());
        registry.set_mdns_active(true);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let state = state(registry, store);

        let topology = topology_of(state).await;

        assert_eq!(topology.discovery.mdns, "active");
    }

    #[tokio::test]
    async fn topology_partitions_a_known_model() {
        let registry = Arc::new(NodeRegistry::new());
        registry.register_or_update(node("w1", 16 * 1024 * 1024 * 1024));
        let store = Arc::new(Store::open_in_memory().unwrap());
        store
            .upsert_model(&NewModel {
                model_id: "llama-8b".into(),
                file_name: "llama.gguf".into(),
                quantization_format: "Q4_K_M".into(),
                total_layers: 32,
                weight_size_bytes: 32 * 1024 * 1024 * 64,
                local_file_path: "/llama.gguf".into(),
            })
            .unwrap();
        let state = state(registry, store);
        state.security.enrollments.enroll("w1", "w1", vec![]);
        // Pairing alone does not prove the worker can hold layers; a fresh
        // health snapshot is what puts it in the plan.
        state
            .telemetry_cache
            .record_worker(crate::telemetry::WorkerHealth {
                id: "w1".into(),
                temp_celsius: None,
                battery_pct: None,
                free_ram_bytes: 1024,
                observed_at_epoch_ms: 1,
                supports_compute: true,
            });

        let topology = topology_of(state).await;

        assert_eq!(topology.status, "active");
        assert_eq!(topology.active_model.as_deref(), Some("llama-8b"));
        // The master coordinates; the paired worker holds every layer.
        let master = topology
            .nodes
            .iter()
            .find(|n| n.role == "master")
            .expect("master listed");
        assert!(master.assigned_layers.is_empty());
        let assigned: usize = topology
            .nodes
            .iter()
            .map(|n| {
                n.assigned_layers
                    .iter()
                    .map(|r| r[1] - r[0] + 1)
                    .sum::<usize>()
            })
            .sum();
        assert_eq!(assigned, 32);
        assert!(topology.used_pooled_ram_mb > 0);
    }

    #[tokio::test]
    async fn benchmark_counts_an_unreachable_node_as_failed_without_a_row() {
        // A node with nothing listening: the probe must be counted, not faked
        // (D-C6). No worker row is written, because nothing was measured.
        let registry = Arc::new(NodeRegistry::new());
        registry.register_or_update(node("w1", 4 * 1024 * 1024 * 1024));
        let store = Arc::new(Store::open_in_memory().unwrap());
        let state = state(registry, store.clone());

        let Json(response) = benchmark(State(state)).await.unwrap();

        assert_eq!(response.nodes_tested, 0, "nothing was measured");
        assert_eq!(response.nodes_failed, 1, "the unreachable node is counted");
        assert_eq!(response.status, "partial");
        assert!(!response.rebalancing_triggered);
        assert_eq!(store.list_benchmarks("w1").unwrap().len(), 0, "no fake row");
    }

    #[tokio::test]
    async fn benchmark_reports_no_nodes_when_nothing_is_discovered() {
        let registry = Arc::new(NodeRegistry::new());
        let store = Arc::new(Store::open_in_memory().unwrap());
        let state = state(registry, store);

        let Json(response) = benchmark(State(state)).await.unwrap();

        assert_eq!(response.status, "no_nodes");
        assert_eq!(response.nodes_tested, 0);
        assert_eq!(response.nodes_failed, 0);
    }

    #[test]
    fn benchmark_status_separates_none_partial_and_success() {
        assert_eq!(benchmark_status(0, 0), "no_nodes");
        assert_eq!(benchmark_status(1, 0), "success");
        assert_eq!(benchmark_status(0, 1), "partial");
        assert_eq!(benchmark_status(2, 1), "partial");
    }

    #[tokio::test]
    async fn stub_benchmarker_is_deterministic_and_node_specific() {
        let benchmarker = StubBenchmarker;
        let a = benchmarker.benchmark(&node("w1", 0)).await.unwrap();
        let b = benchmarker.benchmark(&node("w1", 0)).await.unwrap();
        let c = benchmarker.benchmark(&node("w2", 0)).await.unwrap();
        assert_eq!(a, b);
        assert_ne!(a.memory_bandwidth_mbs, c.memory_bandwidth_mbs);
        assert_eq!(a.network_ping_ms, 1.5);
    }
}
