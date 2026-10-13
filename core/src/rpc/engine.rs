use crate::rpc::client::WorkerRpcClient;
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

/// Orchestrates pipelined forward steps against the pooled worker cluster.
///
/// Each assignment names the device that owns a contiguous layer range; the
/// engine opens a gRPC connection to that device and dispatches the forward
/// pass over the wire. Worker endpoints are addressed as `http://<ip>:<port>`
/// with the port either discovered via mDNS or bridged with ADB.
pub struct RpcPipelineEngine;

impl RpcPipelineEngine {
    pub async fn dispatch_forward_step(
        assignment: &LayerAssignment,
        payload: TensorPayload,
    ) -> Result<ExecutionResult, String> {
        let endpoint = format!("http://{}:{}", assignment.ip_address, assignment.port);
        info!(
            "RPC Engine: dispatching sequence {} (layers {}->{}) to node [{}] ({}) via {}",
            payload.sequence_index,
            assignment.start_layer,
            assignment.end_layer,
            assignment.device_name,
            assignment.node_id,
            endpoint
        );

        let mut client = WorkerRpcClient::connect(&endpoint).await?;
        client.execute_forward(assignment, &payload).await
    }
}