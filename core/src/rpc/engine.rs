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

/// TLS material the driver presents to, and requires from, one worker.
pub struct WorkerTlsConfig {
    /// PEM of the cluster CA that must have signed the worker's server
    /// certificate — anchored by the out-of-band pairing fingerprint.
    pub ca_pem: String,
    /// PEM client certificate the master presents to the worker.
    pub client_cert_pem: String,
    /// PEM private key belonging to `client_cert_pem`.
    pub client_key_pem: String,
}

/// How the driver reaches one worker.
pub enum WorkerTransport {
    /// Mutual TLS, with the worker's certificate pinned to the cluster CA.
    MutualTls(WorkerTlsConfig),
    /// Unauthenticated plaintext; only acceptable on loopback or an ADB tunnel.
    Plaintext,
}

/// Resolves the transport to use for each worker.
///
/// Implemented by the caller (backed by the pairing store) so the driver stays
/// free of any particular credential store. Returning `Err` fails the load
/// *before* any worker is dialed, so an unknown worker is never a downgrade to
/// plaintext.
pub trait WorkerTransportSource: Send + Sync {
    fn transport_for(&self, node_id: &str) -> Result<WorkerTransport, String>;
}

/// The transport source behind [`LoadedPipeline::load`]: every worker plaintext.
struct PlaintextTransports;

impl WorkerTransportSource for PlaintextTransports {
    fn transport_for(&self, _node_id: &str) -> Result<WorkerTransport, String> {
        Ok(WorkerTransport::Plaintext)
    }
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
    /// Loads `assignments` in layer order over plaintext.
    ///
    /// Plaintext is only acceptable on a loopback or ADB-tunneled link; use
    /// [`LoadedPipeline::load_secure`] to reach workers across the network.
    ///
    /// Rejects an empty plan: a pipeline with no stages would silently pass
    /// activations through unmodified. The driver — not the caller — owns
    /// activation order, so the assignments are sorted by `start_layer` before
    /// anything is connected, and the plan is validated (see [`validate_plan`])
    /// *before* any worker is dialed, so an out-of-order, overlapping, gapped or
    /// inverted plan fails fast instead of silently duplicating or skipping
    /// layers.
    pub async fn load(model_id: &str, assignments: &[LayerAssignment]) -> Result<Self, String> {
        Self::load_with(model_id, assignments, &PlaintextTransports).await
    }

    /// Loads `assignments`, dialing each worker with the transport that
    /// `transports` resolves for it.
    ///
    /// Every transport is resolved *before* the first dial, so an incomplete or
    /// misconfigured credential store fails the load without contacting any
    /// worker — the same fail-fast property the plan validation has. There is
    /// deliberately no fallback path: a worker the source reports as
    /// [`WorkerTransport::MutualTls`] is never reached over plaintext, and a
    /// worker the source cannot resolve is an error rather than a downgrade.
    pub async fn load_secure(
        model_id: &str,
        assignments: &[LayerAssignment],
        transports: &dyn WorkerTransportSource,
    ) -> Result<Self, String> {
        Self::load_with(model_id, assignments, transports).await
    }

    async fn load_with(
        model_id: &str,
        assignments: &[LayerAssignment],
        transports: &dyn WorkerTransportSource,
    ) -> Result<Self, String> {
        if assignments.is_empty() {
            return Err("cannot load a pipeline with no layer assignments".into());
        }

        let mut ordered: Vec<&LayerAssignment> = assignments.iter().collect();
        ordered.sort_by_key(|assignment| assignment.start_layer);
        validate_plan(&ordered)?;

        let mut resolved = Vec::with_capacity(ordered.len());
        for assignment in &ordered {
            let transport = transports
                .transport_for(&assignment.node_id)
                .map_err(|error| {
                    format!(
                        "cannot load pipeline: worker {}: {error}",
                        assignment.node_id
                    )
                })?;
            resolved.push(((*assignment).clone(), transport));
        }

        let mut stages = Vec::with_capacity(resolved.len());
        for (assignment, transport) in resolved {
            match load_stage(model_id, &assignment, &transport).await {
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

/// Rejects a plan that cannot describe a sound layer split.
///
/// Expects `ordered` sorted by `start_layer` (as [`LoadedPipeline::load`]
/// guarantees), then enforces three things:
///
/// * every range is well-formed (`start <= end`);
/// * the plan is anchored at layer 0 — a plan is a partition of the model, so
///   starting later would silently skip the model head;
/// * adjacent ranges tile exactly — no overlap (a layer would run twice) and no
///   gap (a layer would be skipped).
///
/// The *tail* is deliberately not checked: the driver is not told how many
/// layers the model has, and `LayerScheduler::compute_plan` may legitimately
/// return a plan the available cluster is too small to finish.
fn validate_plan(ordered: &[&LayerAssignment]) -> Result<(), String> {
    for assignment in ordered {
        if assignment.start_layer > assignment.end_layer {
            return Err(format!(
                "inverted layer range {}..={} for worker {}",
                assignment.start_layer, assignment.end_layer, assignment.node_id
            ));
        }
    }

    if let Some(first) = ordered.first() {
        if first.start_layer != 0 {
            return Err(format!(
                "plan must start at layer 0, but the lowest range starts at {} (worker {})",
                first.start_layer, first.node_id
            ));
        }
    }

    for pair in ordered.windows(2) {
        let (previous, current) = (pair[0], pair[1]);
        match previous.end_layer.checked_add(1) {
            Some(expected) if current.start_layer == expected => {}
            Some(expected) if current.start_layer > expected => {
                return Err(format!(
                    "layer(s) {expected}..={} are not assigned to any worker (between {} and {})",
                    current.start_layer - 1,
                    previous.node_id,
                    current.node_id
                ));
            }
            // `current.start_layer <= previous.end_layer`, or `previous.end_layer`
            // is `usize::MAX` (so no range can start above it).
            _ => {
                return Err(format!(
                    "overlap between {} layers {}..={} and {} layers {}..={}",
                    previous.node_id,
                    previous.start_layer,
                    previous.end_layer,
                    current.node_id,
                    current.start_layer,
                    current.end_layer
                ));
            }
        }
    }

    Ok(())
}

async fn load_stage(
    model_id: &str,
    assignment: &LayerAssignment,
    transport: &WorkerTransport,
) -> Result<PipelineStage, String> {
    let mut client = match transport {
        WorkerTransport::MutualTls(config) => {
            let endpoint = https_endpoint(&assignment.ip_address, assignment.port);
            // The certificate is verified against the address actually dialed,
            // which is the only name the source cannot know: it resolves a
            // transport from the node id alone.
            WorkerRpcClient::connect_tls(
                &endpoint,
                &config.ca_pem,
                &config.client_cert_pem,
                &config.client_key_pem,
                host(&assignment.ip_address),
            )
            .await?
        }
        WorkerTransport::Plaintext => {
            let endpoint = http_endpoint(&assignment.ip_address, assignment.port);
            WorkerRpcClient::connect(&endpoint).await?
        }
    };
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
    endpoint(ip_address, port, "http")
}

/// Builds the `https://host:port` endpoint used by the mTLS transport.
pub fn https_endpoint(ip_address: &str, port: u16) -> String {
    endpoint(ip_address, port, "https")
}

/// The bare host of `ip_address`, without IPv6 brackets.
///
/// The URL builder and the TLS verifier must agree on this: one is compared
/// against the URI authority, the other against a certificate SAN, and a
/// bracketed literal like `[::1]` would match neither as written.
pub fn host(ip_address: &str) -> &str {
    ip_address.trim_start_matches('[').trim_end_matches(']')
}

fn endpoint(ip_address: &str, port: u16, scheme: &str) -> String {
    let host = host(ip_address);
    if host.contains(':') {
        format!("{scheme}://[{host}]:{port}")
    } else {
        format!("{scheme}://{host}:{port}")
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
    fn tls_domain_is_the_bare_host_the_url_uses() {
        // `connect_tls` verifies the certificate against this name, so it has to
        // be the same host the URI authority carries: the bracketed spelling of
        // an IPv6 literal matches no SAN. Both consumers share `host`, so a
        // bracketed assignment cannot verify against `"[::1]"`.
        for (address, expected) in [
            ("192.168.1.50", "192.168.1.50"),
            ("fe80::1", "fe80::1"),
            ("[fe80::1]", "fe80::1"),
            ("::1", "::1"),
            ("[::1]", "::1"),
        ] {
            assert_eq!(host(address), expected, "for {address}");
            // The URL brackets an IPv6 authority while the verifier sees the
            // bare host, so both spell the same name.
            let url = https_endpoint(address, 50052);
            if expected.contains(':') {
                assert!(url.contains(&format!("[{expected}]")), "for {address}");
            }
        }
    }

    #[test]
    fn formats_https_endpoint_and_brackets_ipv6() {
        assert_eq!(
            https_endpoint("192.168.1.50", 50052),
            "https://192.168.1.50:50052"
        );
        assert_eq!(https_endpoint("fe80::1", 50052), "https://[fe80::1]:50052");
        assert_eq!(https_endpoint("[::1]", 50052), "https://[::1]:50052");
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
