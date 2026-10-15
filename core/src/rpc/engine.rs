use crate::rpc::client::{WorkerRpcClient, MASTER_ID, PROTOCOL_VERSION};
use crate::scheduler::planner::LayerAssignment;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationType {
    #[default]
    Fp32,
    Fp16,
    Bf16,
}

impl ActivationType {
    /// Numeric value matching `aiupcycler.rpc.TensorBuffer.DataType`.
    pub fn proto_value(self) -> i32 {
        match self {
            ActivationType::Fp32 => 0,
            ActivationType::Fp16 => 1,
            ActivationType::Bf16 => 2,
        }
    }

    /// Inverse of [`ActivationType::proto_value`]; `None` for an unknown wire
    /// value so callers can fall back rather than guess a tensor format.
    pub fn from_proto_value(value: i32) -> Option<Self> {
        match value {
            0 => Some(ActivationType::Fp32),
            1 => Some(ActivationType::Fp16),
            2 => Some(ActivationType::Bf16),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TensorPayload {
    pub session_id: String,
    pub sequence_index: usize,
    pub shape: Vec<i64>,
    pub data_bytes: Vec<u8>,
    #[serde(default)]
    pub data_type: ActivationType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub session_id: String,
    pub sequence_index: usize,
    pub execution_time_us: u64,
    pub output_bytes: Vec<u8>,
    /// Shape of the activation produced by the last stage, carried so the
    /// pipeline can feed it to the next stage instead of assuming it is
    /// unchanged.
    #[serde(default)]
    pub output_shape: Vec<i64>,
    #[serde(default)]
    pub output_data_type: ActivationType,
}

/// One worker holding a layer range for the lifetime of a pipeline session.
struct PipelineStage {
    assignment: LayerAssignment,
    client: WorkerRpcClient,
}

/// A loaded, connected pipeline: every worker holds its layer range and one
/// activation flows through the stages in ascending layer order.
///
/// `load` connects, handshakes and issues `LoadLayers` on each stage (rolling
/// the already-loaded ones back if any step fails); `forward` threads a single
/// activation through every stage, feeding each stage's output tensor to the
/// next; `unload` consumes the pipeline and releases worker RAM.
///
/// Unlike [`RpcPipelineEngine::dispatch_forward_step`] — a connectionless
/// one-shot used by the desktop debug command — this keeps the workers loaded
/// and connected across many forward passes.
pub struct LoadedPipeline {
    model_id: String,
    stages: Vec<PipelineStage>,
}

impl LoadedPipeline {
    /// Loads `assignments` in layer order. Rejects an empty plan: a pipeline
    /// with no stages would silently pass activations through unmodified.
    pub async fn load(model_id: &str, assignments: &[LayerAssignment]) -> Result<Self, String> {
        if assignments.is_empty() {
            return Err("cannot load a pipeline with no layer assignments".into());
        }

        let mut stages = Vec::with_capacity(assignments.len());
        for assignment in assignments {
            match load_stage(model_id, assignment).await {
                Ok(stage) => stages.push(stage),
                Err(error) => {
                    // Best-effort rollback so a failed load does not pin RAM on
                    // the workers that already succeeded.
                    let _ = unload_all(&mut stages, model_id).await;
                    return Err(error);
                }
            }
        }

        info!(
            "loaded pipeline for model {} across {} stage(s): {}",
            model_id,
            stages.len(),
            describe_stages(&stages)
        );
        Ok(Self {
            model_id: model_id.to_string(),
            stages,
        })
    }

    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }

    /// Threads one activation through every stage and returns the final
    /// tensor plus the summed worker execution time.
    pub async fn forward(&mut self, payload: TensorPayload) -> Result<ExecutionResult, String> {
        if self.stages.is_empty() {
            return Err("pipeline has no loaded stages".into());
        }

        let mut current = payload;
        let mut total_execution_us = 0u64;

        for stage in &mut self.stages {
            let result = stage
                .client
                .execute_forward(&stage.assignment, &current)
                .await?;
            total_execution_us += result.execution_time_us;
            current = TensorPayload {
                session_id: result.session_id,
                sequence_index: result.sequence_index,
                shape: result.output_shape,
                data_bytes: result.output_bytes,
                data_type: result.output_data_type,
            };
        }

        Ok(ExecutionResult {
            session_id: current.session_id,
            sequence_index: current.sequence_index,
            execution_time_us: total_execution_us,
            output_bytes: current.data_bytes,
            output_shape: current.shape,
            output_data_type: current.data_type,
        })
    }

    /// Releases every worker's layer range. Consumes the pipeline so it cannot
    /// be used after its workers have dropped the weights.
    pub async fn unload(mut self) -> Result<(), String> {
        unload_all(&mut self.stages, &self.model_id).await
    }
}

async fn load_stage(model_id: &str, assignment: &LayerAssignment) -> Result<PipelineStage, String> {
    let endpoint = http_endpoint(&assignment.ip_address, assignment.port);
    let mut client = WorkerRpcClient::connect(&endpoint).await?;
    client.handshake(MASTER_ID, PROTOCOL_VERSION).await?;
    let response = client.load_layers(model_id, assignment).await?;
    if !response.success {
        return Err(format!(
            "worker {} refused layers {}..={}: {}",
            assignment.node_id,
            assignment.start_layer,
            assignment.end_layer,
            response.error_message
        ));
    }
    Ok(PipelineStage {
        assignment: assignment.clone(),
        client,
    })
}

async fn unload_all(stages: &mut [PipelineStage], model_id: &str) -> Result<(), String> {
    let mut first_error = None;
    for stage in stages.iter_mut() {
        if let Err(error) = stage.client.unload_layers(model_id).await {
            warn!(
                "failed to unload model {} from worker {}: {error}",
                model_id, stage.assignment.node_id
            );
            first_error.get_or_insert(error);
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn describe_stages(stages: &[PipelineStage]) -> String {
    stages
        .iter()
        .map(|s| {
            format!(
                "{}#{}..{}",
                s.assignment.node_id, s.assignment.start_layer, s.assignment.end_layer
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
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
        let endpoint = http_endpoint(&assignment.ip_address, assignment.port);
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
        client.handshake(MASTER_ID, PROTOCOL_VERSION).await?;
        client.execute_forward(assignment, &payload).await
    }
}

/// Builds an `http://host:port` worker endpoint, bracketing IPv6 literals so
/// they parse as a valid URI authority (e.g. `http://[fe80::1]:50052`).
pub fn http_endpoint(ip_address: &str, port: u16) -> String {
    let host = ip_address.trim_start_matches('[').trim_end_matches(']');
    if host.contains(':') {
        format!("http://[{host}]:{port}")
    } else {
        format!("http://{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_ipv4_endpoint() {
        assert_eq!(
            http_endpoint("192.168.1.50", 50052),
            "http://192.168.1.50:50052"
        );
    }

    #[test]
    fn brackets_ipv6_endpoint() {
        assert_eq!(http_endpoint("fe80::1", 50052), "http://[fe80::1]:50052");
        assert_eq!(http_endpoint("::1", 50052), "http://[::1]:50052");
    }

    #[test]
    fn does_not_double_bracket_ipv6() {
        assert_eq!(http_endpoint("[::1]", 50052), "http://[::1]:50052");
    }

    #[test]
    fn activation_type_round_trips_through_proto_value() {
        for kind in [
            ActivationType::Fp32,
            ActivationType::Fp16,
            ActivationType::Bf16,
        ] {
            assert_eq!(
                ActivationType::from_proto_value(kind.proto_value()),
                Some(kind)
            );
        }
        assert_eq!(ActivationType::from_proto_value(9), None);
    }
}
