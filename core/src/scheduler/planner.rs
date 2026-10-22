use crate::discovery::mdns::DiscoveredNode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

/// The temperature at or above which a worker is considered hot (spec
/// 2026-10-22, D-F2). A policy constant: a phone at 40 °C is plainly warm and
/// throttling becomes likely.
pub const HOT_CELSIUS: f64 = 40.0;

/// The share of its RAM-derived layers a hot worker may take when the cluster
/// cannot spare it a full rest (spec D-F2, D-F3).
pub const HOT_LAYER_SHARE: f64 = 0.5;

/// The last measured temperature of each node, in celsius (spec D-F1).
///
/// Only nodes with a measurement appear here: **unknown is never hot**.
#[derive(Debug, Clone, Default)]
pub struct ThermalHints {
    temperatures: HashMap<String, f64>,
}

impl ThermalHints {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the hints from a node-id → celsius map.
    pub fn from_temperatures(temperatures: HashMap<String, f64>) -> Self {
        Self { temperatures }
    }

    /// Records a measurement for one node.
    pub fn record(&mut self, node_id: impl Into<String>, celsius: f64) {
        self.temperatures.insert(node_id.into(), celsius);
    }

    /// Builder form of [`ThermalHints::record`].
    pub fn with(mut self, node_id: impl Into<String>, celsius: f64) -> Self {
        self.record(node_id, celsius);
        self
    }

    /// The last measured temperature of a node, if it was ever measured.
    pub fn temperature(&self, node_id: &str) -> Option<f64> {
        self.temperatures.get(node_id).copied()
    }

    /// Whether a node is hot (D-F2). A node with no measurement is not.
    pub fn is_hot(&self, node_id: &str) -> bool {
        self.temperatures
            .get(node_id)
            .is_some_and(|celsius| *celsius >= HOT_CELSIUS)
    }

    pub fn is_empty(&self) -> bool {
        self.temperatures.is_empty()
    }
}

/// What the planner did about one node's temperature (spec D-F4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeThermal {
    pub node_id: String,
    /// The temperature the plan was computed against, in celsius.
    pub temp_celsius: f64,
    /// Whether this node's share was limited because it is hot.
    pub capped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    pub model_id: String,
    pub total_layers: usize,
    pub bytes_per_layer: u64,
    pub activation_bytes: u64,
    pub quantization: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerAssignment {
    pub node_id: String,
    pub device_name: String,
    pub ip_address: String,
    pub port: u16,
    pub start_layer: usize,
    pub end_layer: usize,
    pub total_ram_used_bytes: u64,
    pub estimated_latency_ms: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitioningPlan {
    pub model_id: String,
    pub assignments: Vec<LayerAssignment>,
    pub expected_tokens_per_second: f32,
    pub total_layers_assigned: usize,
    /// The thermal state the plan was computed against (spec D-F4).
    pub thermal: Vec<NodeThermal>,
    /// Whether the hot-worker cap had to be relaxed to place the model
    /// (spec D-F3): the cluster was hot enough that resting was not an option.
    pub thermal_relaxed: bool,
}

pub struct LayerScheduler;

/// The one node order both plan callers use: smallest free RAM first, then
/// node id. [`fill_layers`] hands layers out in the order it is given, so a
/// shared order is what stops the topology display and the serving path from
/// disagreeing about which worker holds which range on a mixed cluster.
pub fn order_plan_nodes(nodes: &mut [DiscoveredNode]) {
    nodes.sort_by(|a, b| {
        a.free_ram_bytes
            .cmp(&b.free_ram_bytes)
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
}

impl LayerScheduler {
    /// Solves layer allocation using hardware-aware dynamic programming algorithm
    ///
    /// `thermal` carries the last measured temperature of each node (D-F1). A
    /// hot worker is capped so it can rest, and the cap relaxes only as far as
    /// the cluster needs (D-F2, D-F3): the model is always placed.
    pub fn compute_plan(
        model: &ModelSpec,
        available_nodes: &[DiscoveredNode],
        thermal: &ThermalHints,
    ) -> PartitioningPlan {
        info!(
            "Computing layer partitioning plan for model {} ({} layers, {} MB/layer)",
            model.model_id,
            model.total_layers,
            model.bytes_per_layer / (1024 * 1024)
        );

        if model.bytes_per_layer == 0 {
            warn!(
                "LayerScheduler: model {} declares 0 bytes/layer; cannot partition",
                model.model_id
            );
            return PartitioningPlan {
                model_id: model.model_id.clone(),
                assignments: Vec::new(),
                expected_tokens_per_second: 0.0,
                total_layers_assigned: 0,
                thermal: Vec::new(),
                thermal_relaxed: false,
            };
        }

        // A hot worker rests entirely first, then takes half, then what it must:
        // each step is the minimum the cluster needs (D-F2). With no hot node
        // there is nothing to relax, so only the unrestricted pass runs.
        let any_hot = available_nodes
            .iter()
            .any(|node| thermal.is_hot(&node.node_id));
        let steps: &[f64] = if any_hot {
            &[0.0, HOT_LAYER_SHARE, 1.0]
        } else {
            &[1.0]
        };

        let mut chosen: Option<(Vec<LayerAssignment>, f64)> = None;
        for step in steps {
            let assignments = fill_layers(model, available_nodes, thermal, *step);
            let total = assigned_layers(&assignments);
            chosen = Some((assignments, *step));
            if total == model.total_layers {
                break;
            }
        }
        let (assignments, used_share) = chosen.expect("at least one step runs");

        let total_assigned = assigned_layers(&assignments);
        let est_tok_sec = if total_assigned == model.total_layers {
            8.5
        } else {
            0.0
        };

        // The thermal state the plan used, and whether the cap had to give
        // (D-F3, D-F4).
        let thermal_states: Vec<NodeThermal> = available_nodes
            .iter()
            .filter_map(|node| {
                thermal
                    .temperature(&node.node_id)
                    .map(|temp_celsius| NodeThermal {
                        node_id: node.node_id.clone(),
                        temp_celsius,
                        capped: thermal.is_hot(&node.node_id) && used_share < 1.0,
                    })
            })
            .collect();
        let thermal_relaxed = any_hot && used_share > 0.0;

        PartitioningPlan {
            model_id: model.model_id.clone(),
            assignments,
            expected_tokens_per_second: est_tok_sec,
            total_layers_assigned: total_assigned,
            thermal: thermal_states,
            thermal_relaxed,
        }
    }
}

/// Greedily fills the nodes in the order they are given, with a hot node's
/// share limited to `hot_share` of what its RAM would allow.
fn fill_layers(
    model: &ModelSpec,
    available_nodes: &[DiscoveredNode],
    thermal: &ThermalHints,
    hot_share: f64,
) -> Vec<LayerAssignment> {
    let mut assignments = Vec::new();
    let mut current_layer = 0;

    for node in available_nodes {
        if current_layer >= model.total_layers {
            break;
        }

        // Reserve 20% head-room for OS/KV cache safety buffer, then set
        // aside the activation/KV working set this node must hold while it
        // owns a layer range. Only what remains can hold weight pages.
        let usable_ram = node.free_ram_bytes * 4 / 5;
        let weights_budget = usable_ram.saturating_sub(model.activation_bytes);
        let mut max_layers_for_node = (weights_budget / model.bytes_per_layer) as usize;

        if thermal.is_hot(&node.node_id) {
            max_layers_for_node = (max_layers_for_node as f64 * hot_share).floor() as usize;
        }

        if max_layers_for_node > 0 {
            let layers_to_assign =
                std::cmp::min(max_layers_for_node, model.total_layers - current_layer);

            let start = current_layer;
            let end = start + layers_to_assign - 1;
            let ram_used = (layers_to_assign as u64) * model.bytes_per_layer;

            assignments.push(LayerAssignment {
                node_id: node.node_id.clone(),
                device_name: node.device_name.clone(),
                ip_address: node.ip_address.clone(),
                port: node.port,
                start_layer: start,
                end_layer: end,
                total_ram_used_bytes: ram_used,
                estimated_latency_ms: node.latency_ms * (layers_to_assign as f32),
            });

            current_layer += layers_to_assign;
        }
    }

    assignments
}

/// How many layers a set of assignments covers.
fn assigned_layers(assignments: &[LayerAssignment]) -> usize {
    assignments
        .iter()
        .map(|a| a.end_layer - a.start_layer + 1)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, free_ram_bytes: u64) -> DiscoveredNode {
        DiscoveredNode {
            node_id: id.into(),
            device_name: format!("device-{id}"),
            ip_address: "10.0.0.5".into(),
            port: 50052,
            total_ram_bytes: free_ram_bytes,
            free_ram_bytes,
            connection_type: "WIFI_MDNS".into(),
            latency_ms: 0.5,
            last_seen_epoch_ms: 0,
        }
    }

    #[test]
    fn partitions_layers_across_multiple_capable_nodes() {
        let model = ModelSpec {
            model_id: "llama-3b".into(),
            total_layers: 28,
            bytes_per_layer: 1024 * 1024 * 128,
            activation_bytes: 0,
            quantization: "int4".into(),
        };
        // usable = 80% of free ram -> node1 fits 11 layers, node2 fits 17
        let nodes = vec![
            node("n1", 11 * 1024 * 1024 * 128 * 100 / 80),
            node("n2", 17 * 1024 * 1024 * 128 * 100 / 80),
        ];

        let plan = LayerScheduler::compute_plan(&model, &nodes, &ThermalHints::default());

        assert_eq!(plan.total_layers_assigned, 28);
        assert_eq!(plan.assignments.len(), 2);
        assert_eq!(plan.assignments[0].start_layer, 0);
        assert_eq!(plan.assignments[0].end_layer, 10);
        assert_eq!(plan.assignments[1].start_layer, 11);
        assert_eq!(plan.assignments[1].end_layer, 27);
    }

    #[test]
    fn assignments_carry_device_endpoint_for_rpc() {
        let model = ModelSpec {
            model_id: "tiny".into(),
            total_layers: 2,
            bytes_per_layer: 1024 * 1024,
            activation_bytes: 1024,
            quantization: "fp32".into(),
        };
        let nodes = vec![node("n1", 1024 * 1024 * 1024)];

        let plan = LayerScheduler::compute_plan(&model, &nodes, &ThermalHints::default());
        let assignment = &plan.assignments[0];

        assert_eq!(assignment.ip_address, "10.0.0.5");
        assert_eq!(assignment.port, 50052);
        assert_eq!((assignment.start_layer, assignment.end_layer), (0, 1));
    }

    /// A hot worker rests while the cool ones carry the load (spec D-F2).
    #[test]
    fn a_hot_worker_rests_while_a_cool_one_carries_the_load() {
        let model = ModelSpec {
            model_id: "llama-3b".into(),
            total_layers: 28,
            bytes_per_layer: 1024 * 1024 * 128,
            activation_bytes: 0,
            quantization: "int4".into(),
        };
        // Both nodes could hold the whole model on their own.
        let nodes = vec![
            node("hot", 28 * 1024 * 1024 * 128 * 5 / 4),
            node("cool", 28 * 1024 * 1024 * 128 * 5 / 4),
        ];
        let hints = ThermalHints::default().with("hot", 46.0).with("cool", 30.0);

        let plan = LayerScheduler::compute_plan(&model, &nodes, &hints);

        assert_eq!(plan.total_layers_assigned, 28, "the model still fits");
        assert!(
            plan.assignments.iter().all(|a| a.node_id != "hot"),
            "the hot worker must rest, got {:?}",
            plan.assignments
        );
        assert!(!plan.thermal_relaxed, "resting was enough");
        let hot = plan
            .thermal
            .iter()
            .find(|t| t.node_id == "hot")
            .expect("the hot node is reported");
        assert!(hot.capped, "the hot node is reported as capped");
        assert_eq!(hot.temp_celsius, 46.0);
        let cool = plan
            .thermal
            .iter()
            .find(|t| t.node_id == "cool")
            .expect("the cool node is reported");
        assert!(!cool.capped, "a cool node is never capped");
    }

    /// The cap relaxes only as far as the cluster needs (spec D-F2/D-F3).
    #[test]
    fn a_hot_worker_takes_half_when_the_cluster_cannot_do_without_it() {
        let model = ModelSpec {
            model_id: "llama-3b".into(),
            total_layers: 28,
            bytes_per_layer: 1024 * 1024 * 128,
            activation_bytes: 0,
            quantization: "int4".into(),
        };
        // The cool node holds 21 layers, the hot one 14: resting entirely
        // would leave 7 layers unplaced, half of the hot node's share fits.
        let nodes = vec![
            node("hot", 14 * 1024 * 1024 * 128 * 5 / 4),
            node("cool", 21 * 1024 * 1024 * 128 * 5 / 4),
        ];
        let hints = ThermalHints::default().with("hot", 46.0).with("cool", 30.0);

        let plan = LayerScheduler::compute_plan(&model, &nodes, &hints);

        assert_eq!(plan.total_layers_assigned, 28, "the model still fits");
        assert!(plan.thermal_relaxed, "the resting step did not fit");
        let hot = plan
            .assignments
            .iter()
            .find(|a| a.node_id == "hot")
            .expect("the hot worker takes part of the load");
        assert_eq!(hot.end_layer - hot.start_layer + 1, 7, "half of its share");
    }

    /// Unknown is never hot (spec D-F1).
    #[test]
    fn a_worker_without_a_measurement_is_never_capped() {
        let model = ModelSpec {
            model_id: "tiny".into(),
            total_layers: 4,
            bytes_per_layer: 1024 * 1024,
            activation_bytes: 0,
            quantization: "int4".into(),
        };
        let nodes = vec![node("n1", 4 * 1024 * 1024 * 5 / 4)];

        let plan = LayerScheduler::compute_plan(&model, &nodes, &ThermalHints::default());

        assert_eq!(plan.total_layers_assigned, 4);
        assert_eq!(plan.assignments[0].node_id, "n1");
        assert!(plan.thermal.is_empty(), "nothing was measured");
        assert!(!plan.thermal_relaxed);
    }

    /// A cluster that is entirely hot still places the model (spec D-F3).
    #[test]
    fn an_entirely_hot_cluster_still_places_the_model() {
        let model = ModelSpec {
            model_id: "llama-3b".into(),
            total_layers: 28,
            bytes_per_layer: 1024 * 1024 * 128,
            activation_bytes: 0,
            quantization: "int4".into(),
        };
        let nodes = vec![
            node("n1", 14 * 1024 * 1024 * 128 * 5 / 4),
            node("n2", 14 * 1024 * 1024 * 128 * 5 / 4),
        ];
        let hints = ThermalHints::default().with("n1", 48.0).with("n2", 47.0);

        let plan = LayerScheduler::compute_plan(&model, &nodes, &hints);

        assert_eq!(plan.total_layers_assigned, 28, "the model still fits");
        assert!(plan.thermal_relaxed, "the cap had to be relaxed");
        assert!(
            plan.thermal.iter().all(|t| !t.capped),
            "a node that took what it must is not reported as capped"
        );
    }

    #[test]
    fn leaves_no_work_unassigned_when_nodes_are_insufficient() {
        let model = ModelSpec {
            model_id: "big".into(),
            total_layers: 100,
            bytes_per_layer: 1024 * 1024 * 256,
            activation_bytes: 1024 * 1024,
            quantization: "int8".into(),
        };
        // 1 GiB free -> 3 usable layers per node
        let nodes = vec![
            node("n1", 1024 * 1024 * 1024),
            node("n2", 1024 * 1024 * 1024),
        ];

        let plan = LayerScheduler::compute_plan(&model, &nodes, &ThermalHints::default());

        assert_eq!(plan.total_layers_assigned, 6);
        assert_eq!(plan.expected_tokens_per_second, 0.0);
    }

    #[test]
    fn activation_working_set_reduces_layer_capacity() {
        // ~858 MB usable: 3 layers of 256 MiB fit on weights alone, but a
        // 300 MiB activation budget leaves room for only 2.
        let model = ModelSpec {
            model_id: "kv-heavy".into(),
            total_layers: 10,
            bytes_per_layer: 256 * 1024 * 1024,
            activation_bytes: 300 * 1024 * 1024,
            quantization: "fp16".into(),
        };
        let nodes = vec![node("n1", 1024 * 1024 * 1024)];

        let plan = LayerScheduler::compute_plan(&model, &nodes, &ThermalHints::default());

        assert_eq!(plan.total_layers_assigned, 2);
    }

    #[test]
    fn zero_bytes_per_layer_yields_empty_plan_without_panicking() {
        let model = ModelSpec {
            model_id: "malformed".into(),
            total_layers: 10,
            bytes_per_layer: 0,
            activation_bytes: 1024,
            quantization: "fp32".into(),
        };
        let nodes = vec![node("n1", 1024 * 1024 * 1024)];

        let plan = LayerScheduler::compute_plan(&model, &nodes, &ThermalHints::default());

        assert_eq!(plan.total_layers_assigned, 0);
        assert!(plan.assignments.is_empty());
        assert_eq!(plan.expected_tokens_per_second, 0.0);
    }

    /// The 20% head-room the scheduler reserves for the OS/KV cache safety
    /// buffer is a budget, not a suggestion: no assignment may exceed it. This
    /// sweeps node capacity, layer size and activation working set rather than
    /// asserting one hand-picked case, and reports the RAM figure must match
    /// the owned range (the dashboard surfaces it).
    #[test]
    fn never_breaches_the_eighty_percent_head_room_budget() {
        let free_ram_values = [0u64, 1, 1024, 1 << 20, 1 << 30, 8 << 30];
        let layer_bytes = [1u64, 1024, 1 << 20, 128 << 20];
        let activation_values = [0u64, 1 << 20, 300 << 20];

        let mut plans_with_assignments = 0usize;
        let mut plans_without_assignments = 0usize;

        for &free_ram in &free_ram_values {
            for &bytes_per_layer in &layer_bytes {
                for &activation_bytes in &activation_values {
                    for total_layers in [1usize, 7, 28] {
                        let model = ModelSpec {
                            model_id: "sweep".into(),
                            total_layers,
                            bytes_per_layer,
                            activation_bytes,
                            quantization: "int4".into(),
                        };
                        let nodes = vec![node("n1", free_ram), node("n2", free_ram)];
                        let plan =
                            LayerScheduler::compute_plan(&model, &nodes, &ThermalHints::default());
                        let budget = free_ram * 4 / 5;

                        if plan.assignments.is_empty() {
                            plans_without_assignments += 1;
                        } else {
                            plans_with_assignments += 1;
                        }

                        for assignment in &plan.assignments {
                            let owned_layers = assignment.end_layer - assignment.start_layer + 1;
                            assert_eq!(
                                assignment.total_ram_used_bytes,
                                owned_layers as u64 * bytes_per_layer,
                                "reported RAM must match the owned range: {assignment:?}"
                            );
                            assert!(
                                assignment.total_ram_used_bytes <= budget,
                                "free={free_ram} layer={bytes_per_layer} act={activation_bytes}: \
                                 {} bytes exceeds the 80% budget of {budget}",
                                assignment.total_ram_used_bytes
                            );
                        }
                    }
                }
            }
        }

        // Guard against a sweep that silently stopped exercising the logic.
        assert!(
            plans_with_assignments > 0,
            "sweep was vacuous: no plan ever assigned a layer"
        );
        assert!(
            plans_without_assignments > 0,
            "sweep never exercised the zero-capacity path"
        );
    }

    /// `LoadedPipeline` refuses any plan that is not anchored at layer 0 and
    /// tiled without gaps or overlaps. That is only safe if the scheduler
    /// always emits such a plan, so lock the producer side down too.
    #[test]
    fn plan_is_anchored_and_contiguous_for_the_split_driver() {
        let free_ram_values = [0u64, 1024, 3 * 1024 * 1024, 1 << 30, 96 << 30];

        for &free_ram in &free_ram_values {
            let model = ModelSpec {
                model_id: "split".into(),
                total_layers: 32,
                bytes_per_layer: 1024 * 1024,
                activation_bytes: 4096,
                quantization: "int4".into(),
            };
            let nodes = vec![
                node("n1", free_ram),
                node("n2", free_ram),
                node("n3", free_ram),
            ];
            let plan = LayerScheduler::compute_plan(&model, &nodes, &ThermalHints::default());

            if let Some(first) = plan.assignments.first() {
                assert_eq!(
                    first.start_layer, 0,
                    "free={free_ram}: the plan must be anchored at layer 0"
                );
            }

            let mut next_expected = 0usize;
            for assignment in &plan.assignments {
                assert_eq!(
                    assignment.start_layer, next_expected,
                    "free={free_ram}: ranges must tile without a gap or overlap: {assignment:?}"
                );
                assert!(assignment.end_layer >= assignment.start_layer);
                next_expected = assignment.end_layer + 1;
            }

            assert_eq!(plan.total_layers_assigned, next_expected);
            assert!(plan.total_layers_assigned <= model.total_layers);
        }
    }

    /// One order, both plan callers (topology display and serving): smallest
    /// free RAM first, then node id, independent of the input order.
    #[test]
    fn plan_nodes_are_ordered_smallest_ram_first_then_by_id() {
        let mut nodes = vec![
            node("big", 8 << 30),
            node("small", 1 << 30),
            node("mid", 4 << 30),
        ];
        order_plan_nodes(&mut nodes);
        let ids: Vec<&str> = nodes.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(ids, vec!["small", "mid", "big"]);

        let mut same_ram = vec![node("b", 2 << 30), node("a", 2 << 30)];
        order_plan_nodes(&mut same_ram);
        let ids: Vec<&str> = same_ram.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["a", "b"],
            "ties break on node id, never on map order"
        );
    }
}
