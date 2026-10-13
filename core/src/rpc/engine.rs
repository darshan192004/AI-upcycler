use crate::scheduler::planner::LayerAssignment;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TensorPayload {
    pub session_id: String,
    pub sequence_index: usize,
    pub shape: Vec<i64>,
    pub data_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub session_id: String,
    pub sequence_index: usize,
    pub execution_time_us: u64,
    pub output_bytes: Vec<u8>,
}

pub struct RpcPipelineEngine;

impl RpcPipelineEngine {
    pub async fn dispatch_forward_step(
        assignment: &LayerAssignment,
        payload: TensorPayload,
    ) -> Result<ExecutionResult, String> {
        info!(
            "RPC Engine: Dispatching sequence {} (layers {}->{}) to node [{}] ({})",
            payload.sequence_index,
            assignment.start_layer,
            assignment.end_layer,
            assignment.device_name,
            assignment.node_id
        );

        // Simulated zero-copy RPC forward execution timing
        let exec_time = 12_500 + (assignment.end_layer - assignment.start_layer + 1) as u64 * 850;

        Ok(ExecutionResult {
            session_id: payload.session_id,
            sequence_index: payload.sequence_index,
            execution_time_us: exec_time,
            output_bytes: payload.data_bytes,
        })
    }
}
