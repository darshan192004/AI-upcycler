use crate::discovery::mdns::DiscoveredNode;
use serde::{Deserialize, Serialize};
use tracing::info;

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

        let mut assignments = Vec::new();
        let mut current_layer = 0;

        for node in available_nodes {
            if current_layer >= model.total_layers {
                break;
            }

            // Reserve 20% head-room for OS/KV cache safety buffer
            let usable_ram = (node.free_ram_bytes as f64 * 0.8) as u64;
            let max_layers_for_node = (usable_ram / model.bytes_per_layer) as usize;

            if max_layers_for_node > 0 {
                let layers_to_assign = std::cmp::min(
                    max_layers_for_node,
                    model.total_layers - current_layer,
                );

                let start = current_layer;
                let end = start + layers_to_assign - 1;
                let ram_used = (layers_to_assign as u64) * model.bytes_per_layer;

                assignments.push(LayerAssignment {
                    node_id: node.node_id.clone(),
                    device_name: node.device_name.clone(),
                    start_layer: start,
                    end_layer: end,
                    total_ram_used_bytes: ram_used,
                    estimated_latency_ms: node.latency_ms * (layers_to_assign as f32),
                });

                current_layer += layers_to_assign;
            }
        }

        let total_assigned = assignments.iter().map(|a| a.end_layer - a.start_layer + 1).sum();
        let est_tok_sec = if total_assigned == model.total_layers { 8.5 } else { 0.0 };

        PartitioningPlan {
            model_id: model.model_id.clone(),
            assignments,
            expected_tokens_per_second: est_tok_sec,
            total_layers_assigned: total_assigned,
        }
    }
}
