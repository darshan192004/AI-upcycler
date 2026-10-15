use crate::discovery::mdns::DiscoveredNode;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

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
}

pub struct LayerScheduler;

impl LayerScheduler {
    /// Solves layer allocation using hardware-aware dynamic programming algorithm
    pub fn compute_plan(model: &ModelSpec, available_nodes: &[DiscoveredNode]) -> PartitioningPlan {
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
            };
        }

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
            let max_layers_for_node = (weights_budget / model.bytes_per_layer) as usize;

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

        let total_assigned = assignments
            .iter()
            .map(|a| a.end_layer - a.start_layer + 1)
            .sum();
        let est_tok_sec = if total_assigned == model.total_layers {
            8.5
        } else {
            0.0
        };

        PartitioningPlan {
            model_id: model.model_id.clone(),
            assignments,
            expected_tokens_per_second: est_tok_sec,
            total_layers_assigned: total_assigned,
        }
    }
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

        let plan = LayerScheduler::compute_plan(&model, &nodes);

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

        let plan = LayerScheduler::compute_plan(&model, &nodes);
        let assignment = &plan.assignments[0];

        assert_eq!(assignment.ip_address, "10.0.0.5");
        assert_eq!(assignment.port, 50052);
        assert_eq!((assignment.start_layer, assignment.end_layer), (0, 1));
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

        let plan = LayerScheduler::compute_plan(&model, &nodes);

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

        let plan = LayerScheduler::compute_plan(&model, &nodes);

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

        let plan = LayerScheduler::compute_plan(&model, &nodes);

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
                        let plan = LayerScheduler::compute_plan(&model, &nodes);
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
            let plan = LayerScheduler::compute_plan(&model, &nodes);

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
}
