use crate::rpc::engine::{ExecutionResult, TensorPayload};
use crate::rpc::proto::aiupcycler::rpc::{
    worker_service_client::WorkerServiceClient, ExecuteForwardRequest, HandshakeRequest,
    HandshakeResponse, HealthRequest, HealthResponse, TensorBuffer,
};
use crate::scheduler::planner::LayerAssignment;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;

/// Identity this master node presents during a worker handshake.
pub const MASTER_ID: &str = "ai-upcycler-master";

/// Wire protocol version spoken by this build.
pub const PROTOCOL_VERSION: &str = "1.0";

/// Maximum time to establish a TCP/gRPC connection to a worker.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum time for a single RPC call once connected.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// gRPC client for a single AI Upcycler worker node.
///
/// Talks over `tonic` to a worker exposing the `aiupcycler.rpc.WorkerService`
/// contract compiled from `proto/cluster_rpc.proto`.
pub struct WorkerRpcClient {
    inner: WorkerServiceClient<tonic::transport::Channel>,
}

/// Maps an engine-level forward step onto the wire request type defined by
/// `proto/cluster_rpc.proto`. Kept as a pure function so it is unit-testable
/// without a live worker node.
pub fn build_execute_request(
    assignment: &LayerAssignment,
    payload: &TensorPayload,
) -> ExecuteForwardRequest {
    ExecuteForwardRequest {
        session_id: payload.session_id.clone(),
        start_layer: assignment.start_layer as i32,
        end_layer: assignment.end_layer as i32,
        activation_tensor: Some(TensorBuffer {
            shape: payload.shape.clone(),
            data_type: payload.data_type.proto_value(),
            raw_data: payload.data_bytes.clone(),
        }),
        sequence_position: payload.sequence_index as i32,
    }
}

impl WorkerRpcClient {
    /// Connects to a worker at `http://<host>:<port>`.
    pub async fn connect(addr: &str) -> Result<Self, String> {
        let endpoint = tonic::transport::Endpoint::new(addr.to_string())
            .map_err(|e| format!("invalid RPC endpoint '{addr}': {e}"))?
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT);
        let inner = WorkerServiceClient::connect(endpoint)
            .await
            .map_err(|e| format!("failed to connect to worker at '{addr}': {e}"))?;
        Ok(Self { inner })
    }

    /// Authenticates an RPC session with the worker and reads back its device
    /// capabilities.
    pub async fn handshake(
        &mut self,
        master_id: &str,
        protocol_version: &str,
    ) -> Result<HandshakeResponse, String> {
        let request = HandshakeRequest {
            master_id: master_id.to_string(),
            protocol_version: protocol_version.to_string(),
            timestamp: unix_timestamp_secs(),
        };
        self.inner
            .handshake(request)
            .await
            .map_err(|e| format!("handshake failed: {e}"))
            .map(|r| r.into_inner())
    }

    /// Pushes one forward pass of the layer range owned by `assignment`
    /// through the worker and returns the resulting activation tensor.
    pub async fn execute_forward(
        &mut self,
        assignment: &LayerAssignment,
        payload: &TensorPayload,
    ) -> Result<ExecutionResult, String> {
        let request = build_execute_request(assignment, payload);
        info!(
            "RPC client: execute_forward session {} sequence {} layers {}..={}",
            request.session_id, payload.sequence_index, request.start_layer, request.end_layer
        );
        let response = self
            .inner
            .execute_forward(request)
            .await
            .map_err(|e| format!("execute_forward failed: {e}"))?
            .into_inner();

        if !response.success {
            return Err(format!(
                "worker rejected forward step for session {} (layers {}..={})",
                payload.session_id, assignment.start_layer, assignment.end_layer
            ));
        }

        Ok(ExecutionResult {
            session_id: payload.session_id.clone(),
            sequence_index: payload.sequence_index,
            execution_time_us: response.execution_time_us as u64,
            output_bytes: response
                .output_activation_tensor
                .map(|t| t.raw_data)
                .unwrap_or_default(),
        })
    }

    /// Polls the worker's battery/thermal/RAM health snapshot.
    pub async fn health(&mut self) -> Result<HealthResponse, String> {
        let response = self
            .inner
            .get_health_status(HealthRequest {})
            .await
            .map_err(|e| format!("get_health_status failed: {e}"))?
            .into_inner();
        Ok(response)
    }
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::engine::{ActivationType, TensorPayload};
    use crate::scheduler::planner::LayerAssignment;

    fn sample_assignment() -> LayerAssignment {
        LayerAssignment {
            node_id: "android-phone-01".into(),
            device_name: "Galaxy S10".into(),
            ip_address: "127.0.0.1".into(),
            port: 50052,
            start_layer: 8,
            end_layer: 15,
            total_ram_used_bytes: 1536 * 1024 * 1024,
            estimated_latency_ms: 4.2,
        }
    }

    fn sample_payload() -> TensorPayload {
        TensorPayload {
            session_id: "session-1".into(),
            sequence_index: 3,
            shape: vec![1, 4, 4096],
            data_bytes: vec![0u8, 1, 2, 3, 4, 5, 6, 7],
            data_type: ActivationType::Fp32,
        }
    }

    #[test]
    fn maps_assignment_and_payload_to_proto_request() {
        let req = build_execute_request(&sample_assignment(), &sample_payload());

        assert_eq!(req.session_id, "session-1");
        assert_eq!(req.start_layer, 8);
        assert_eq!(req.end_layer, 15);
        assert_eq!(req.sequence_position, 3);

        let tensor = req.activation_tensor.expect("tensor present");
        assert_eq!(tensor.shape, vec![1, 4, 4096]);
        assert_eq!(tensor.raw_data, vec![0u8, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(tensor.data_type, 0, "FP32");
    }

    #[test]
    fn preserves_quantized_activation_data_type() {
        let payload = TensorPayload {
            data_type: ActivationType::Fp16,
            ..sample_payload()
        };
        let req = build_execute_request(&sample_assignment(), &payload);
        assert_eq!(req.activation_tensor.unwrap().data_type, 1, "FP16");
    }

    #[test]
    fn layer_range_is_inclusive() {
        let a = LayerAssignment {
            start_layer: 0,
            end_layer: 11,
            ..sample_assignment()
        };
        let req = build_execute_request(&a, &sample_payload());
        assert_eq!(req.start_layer, 0);
        assert_eq!(req.end_layer, 11);
    }
}
