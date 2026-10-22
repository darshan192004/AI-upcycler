use crate::rpc::engine::{ActivationType, ExecutionResult, TensorPayload};
use crate::rpc::proto::aiupcycler::rpc::{
    worker_service_client::WorkerServiceClient, BenchmarkRequest, BenchmarkResponse,
    ExecuteForwardRequest, HandshakeRequest, HandshakeResponse, HealthRequest, HealthResponse,
    LoadLayersRequest, LoadLayersResponse, TensorBuffer, UnloadLayersRequest, UnloadLayersResponse,
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
///
/// Measured (2026-10-18, release profile, this laptop + the real
/// Qwen2.5-0.5B Q4_K_M model): one full prefill forward through the
/// single-stage plan takes ~30s, and a DEBUG-build forward exceeds 30s
/// outright (the old limit killed it - see `split_equivalence` runs). A
/// phone is slower than this laptop, so prefill and long decode steps must
/// not be murdered by the deadline; 120s stays bounded.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// RPC message limits, both directions. The head stage of a plan returns
/// WHOLE prefill logits (`[n_tokens, n_vocab]` f32); for the shipped model
/// one vocabulary row is 607,744 bytes(!), so a ~30-token prompt is already
/// ~7.9MB - far past tonic's default 4MB. 256MiB admits prefill of ~430
/// tokens on the shipped model; long-context serving needs a last-row-only
/// wire contract (a proto change, deferred per the epic's D6) and the
/// master refuses prompts that would not fit BEFORE dialing.
pub const MAX_ACTIVATION_WIRE_BYTES: usize = 256 * 1024 * 1024;

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
    last_logits_only: bool,
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
        last_logits_only,
    }
}

/// Maps a layer-range assignment onto the wire `LoadLayersRequest`. The weight
/// bytes are resolved worker-side for now; `weights_chunk_data` stays empty
/// until the GGUF slicer can push a contiguous chunk.
pub fn build_load_request(model_id: &str, assignment: &LayerAssignment) -> LoadLayersRequest {
    LoadLayersRequest {
        model_id: model_id.to_string(),
        start_layer: assignment.start_layer as i32,
        end_layer: assignment.end_layer as i32,
        weights_chunk_data: Vec::new(),
    }
}

impl WorkerRpcClient {
    /// Connects to a worker at `http://<host>:<port>` over plaintext
    /// (loopback/ADB-tunneled transports only).
    pub async fn connect(addr: &str) -> Result<Self, String> {
        let endpoint = tonic::transport::Endpoint::new(addr.to_string())
            .map_err(|e| format!("invalid RPC endpoint '{addr}': {e}"))?
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT);
        let inner = WorkerServiceClient::connect(endpoint)
            .await
            .map_err(|e| format!("failed to connect to worker at '{addr}': {e}"))?
            .max_decoding_message_size(MAX_ACTIVATION_WIRE_BYTES)
            .max_encoding_message_size(MAX_ACTIVATION_WIRE_BYTES);
        Ok(Self { inner })
    }

    /// Connects to a worker at `https://<host>:<port>` over mutual TLS.
    ///
    /// `ca_pem` anchors the worker's server certificate (the master validates
    /// the worker's fingerprint out-of-band via the pairing QR payload);
    /// `client_cert_pem`/`client_key_pem` are the master's client credentials.
    /// `domain` must match a SAN on the worker's server certificate: the split
    /// driver passes the address it dials (`LayerAssignment::ip_address`), which
    /// is the SAN the worker was issued during pairing.
    pub async fn connect_tls(
        addr: &str,
        ca_pem: &str,
        client_cert_pem: &str,
        client_key_pem: &str,
        domain: &str,
    ) -> Result<Self, String> {
        let tls = tonic::transport::ClientTlsConfig::new()
            .ca_certificate(tonic::transport::Certificate::from_pem(ca_pem))
            .identity(tonic::transport::Identity::from_pem(
                client_cert_pem,
                client_key_pem,
            ))
            .domain_name(domain);
        let endpoint = tonic::transport::Endpoint::new(addr.to_string())
            .map_err(|e| format!("invalid RPC endpoint '{addr}': {e}"))?
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .tls_config(tls)
            .map_err(|e| format!("invalid TLS config for '{addr}': {e}"))?;
        let inner = WorkerServiceClient::connect(endpoint)
            .await
            .map_err(|e| format!("failed to connect to worker at '{addr}' over mTLS: {e}"))?
            .max_decoding_message_size(MAX_ACTIVATION_WIRE_BYTES)
            .max_encoding_message_size(MAX_ACTIVATION_WIRE_BYTES);
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
    ///
    /// `last_logits_only` is the wire hint for a head stage: the worker answers
    /// `[1, n_vocab]` instead of `[n, n_vocab]` (spec 2026-10-21-last-row-logits,
    /// L2). Workers without the capability ignore it and return whole logits.
    pub async fn execute_forward(
        &mut self,
        assignment: &LayerAssignment,
        payload: &TensorPayload,
        last_logits_only: bool,
    ) -> Result<ExecutionResult, String> {
        let request = build_execute_request(assignment, payload, last_logits_only);
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

        let output = response.output_activation_tensor.unwrap_or_default();
        Ok(ExecutionResult {
            session_id: payload.session_id.clone(),
            sequence_index: payload.sequence_index,
            execution_time_us: response.execution_time_us as u64,
            output_bytes: output.raw_data,
            // An absent/empty tensor means the worker echoed nothing useful;
            // fall back to the request's shape and dtype rather than emit a
            // zero-dimensional tensor.
            output_shape: if output.shape.is_empty() {
                payload.shape.clone()
            } else {
                output.shape
            },
            output_data_type: ActivationType::from_proto_value(output.data_type)
                .unwrap_or(payload.data_type),
        })
    }

    /// Instructs the worker to load the layer range owned by `assignment` for
    /// `model_id`. Returns the raw response so callers can read the allocated
    /// RAM figure or a rejection message.
    pub async fn load_layers(
        &mut self,
        model_id: &str,
        assignment: &LayerAssignment,
    ) -> Result<LoadLayersResponse, String> {
        let request = build_load_request(model_id, assignment);
        info!(
            "RPC client: load_layers model {} layers {}..={}",
            model_id, request.start_layer, request.end_layer
        );
        self.inner
            .load_layers(request)
            .await
            .map_err(|e| format!("load_layers failed: {e}"))
            .map(|r| r.into_inner())
    }

    /// Releases the worker's copy of `model_id`, freeing the RAM it held.
    pub async fn unload_layers(&mut self, model_id: &str) -> Result<UnloadLayersResponse, String> {
        let request = UnloadLayersRequest {
            model_id: model_id.to_string(),
        };
        info!("RPC client: unload_layers model {model_id}");
        self.inner
            .unload_layers(request)
            .await
            .map_err(|e| format!("unload_layers failed: {e}"))
            .map(|r| r.into_inner())
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

    /// Asks the worker to measure its own memory bandwidth and compute
    /// throughput (spec 2026-10-21, D-C1/D-C2). A pre-probe worker answers
    /// `UNIMPLEMENTED`, which the benchmark handler counts rather than fakes.
    pub async fn benchmark(&mut self) -> Result<BenchmarkResponse, String> {
        let response = self
            .inner
            .benchmark(BenchmarkRequest {})
            .await
            .map_err(|e| format!("benchmark failed: {e}"))?
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
        let req = build_execute_request(&sample_assignment(), &sample_payload(), false);

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
        let req = build_execute_request(&sample_assignment(), &payload, false);
        assert_eq!(req.activation_tensor.unwrap().data_type, 1, "FP16");
    }

    #[test]
    fn layer_range_is_inclusive() {
        let a = LayerAssignment {
            start_layer: 0,
            end_layer: 11,
            ..sample_assignment()
        };
        let req = build_execute_request(&a, &sample_payload(), false);
        assert_eq!(req.start_layer, 0);
        assert_eq!(req.end_layer, 11);
    }

    #[test]
    fn maps_assignment_to_load_request() {
        let req = build_load_request("llama-3-8b", &sample_assignment());
        assert_eq!(req.model_id, "llama-3-8b");
        assert_eq!(req.start_layer, 8);
        assert_eq!(req.end_layer, 15);
        assert!(
            req.weights_chunk_data.is_empty(),
            "weights are resolved worker-side until the GGUF slicer pushes chunks"
        );
    }
}
