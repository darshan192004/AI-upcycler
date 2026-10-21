//! Test doubles shared by the driver and API integration tests.
//!
//! `MockWorker` stands in for the not-yet-built native engine: it records every
//! RPC into a shared log and appends a unique tag byte to the activation it
//! receives, so a test can prove the exact cross-worker order the driver
//! produced. Not every helper is used by every test binary, hence the blanket
//! `dead_code` allowance.
#![allow(dead_code)]

use ai_upcycler_core::rpc::engine::{ActivationType, LoadedPipeline, TensorPayload};
use ai_upcycler_core::rpc::proto::aiupcycler::rpc::worker_service_server::{
    WorkerService, WorkerServiceServer,
};
use ai_upcycler_core::rpc::proto::aiupcycler::rpc::{
    BenchmarkRequest, BenchmarkResponse, ExecuteForwardRequest, ExecuteForwardResponse,
    HandshakeRequest, HandshakeResponse, HealthRequest, HealthResponse, LoadLayersRequest,
    LoadLayersResponse, TensorBuffer, UnloadLayersRequest, UnloadLayersResponse,
};
use ai_upcycler_core::scheduler::planner::LayerAssignment;
use ai_upcycler_core::security::{NodeCa, ServerIdentity};
use std::sync::{Arc, Mutex};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};

/// Shared ordered log of the RPCs a mock worker received.
pub type Events = Arc<Mutex<Vec<String>>>;

/// Deterministic stand-in for the native worker engine.
///
/// * `handshake` reports a fixed identity.
/// * `load_layers` records the range; `refusing_load` makes it refuse.
/// * `execute_forward` appends `tag` to the incoming activation and reports a
///   fixed latency; `refusing_forward` makes it reject.
/// * `unload_layers` records the release.
///
/// Every call is appended to the shared `events` log so a test can assert the
/// exact cross-worker order the driver produced.
pub struct MockWorker {
    node_id: String,
    tag: u8,
    execution_time_us: i64,
    fail_load: bool,
    fail_forward: bool,
    /// When false, `benchmark` answers `UNIMPLEMENTED` — the shape every
    /// pre-benchmark-probe worker presents to the master (spec 2026-10-21).
    serve_benchmark: bool,
    events: Events,
}

impl MockWorker {
    pub fn new(
        node_id: &str,
        tag: u8,
        execution_time_us: i64,
        events: Arc<Mutex<Vec<String>>>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            tag,
            execution_time_us,
            fail_load: false,
            fail_forward: false,
            serve_benchmark: true,
            events,
        }
    }

    pub fn refusing_load(mut self) -> Self {
        self.fail_load = true;
        self
    }

    pub fn refusing_forward(mut self) -> Self {
        self.fail_forward = true;
        self
    }

    /// Makes this worker answer `UNIMPLEMENTED` to `Benchmark`, like every
    /// worker built before the probe RPC existed.
    pub fn without_benchmark(mut self) -> Self {
        self.serve_benchmark = false;
        self
    }

    fn record(&self, event: impl Into<String>) {
        self.events.lock().expect("events lock").push(event.into());
    }
}

#[tonic::async_trait]
impl WorkerService for MockWorker {
    async fn handshake(
        &self,
        _request: tonic::Request<HandshakeRequest>,
    ) -> Result<tonic::Response<HandshakeResponse>, tonic::Status> {
        self.record(format!("handshake:{}", self.node_id));
        Ok(tonic::Response::new(HandshakeResponse {
            worker_id: self.node_id.clone(),
            device_name: format!("{} device", self.node_id),
            total_ram_bytes: 1 << 30,
            free_ram_bytes: 1 << 29,
            cpu_cores: 4,
            has_npu_acceleration: false,
        }))
    }

    async fn load_layers(
        &self,
        request: tonic::Request<LoadLayersRequest>,
    ) -> Result<tonic::Response<LoadLayersResponse>, tonic::Status> {
        let request = request.into_inner();
        self.record(format!(
            "load:{}:{}..{}",
            self.node_id, request.start_layer, request.end_layer
        ));
        if self.fail_load {
            return Ok(tonic::Response::new(LoadLayersResponse {
                success: false,
                error_message: format!("{} has no free RAM", self.node_id),
                allocated_ram_bytes: 0,
            }));
        }
        Ok(tonic::Response::new(LoadLayersResponse {
            success: true,
            error_message: String::new(),
            allocated_ram_bytes: 1 << 20,
        }))
    }

    async fn execute_forward(
        &self,
        request: tonic::Request<ExecuteForwardRequest>,
    ) -> Result<tonic::Response<ExecuteForwardResponse>, tonic::Status> {
        let request = request.into_inner();
        self.record(format!("forward:{}", self.node_id));
        if self.fail_forward {
            return Ok(tonic::Response::new(ExecuteForwardResponse {
                success: false,
                output_activation_tensor: None,
                execution_time_us: 0,
            }));
        }

        let tensor = request.activation_tensor.unwrap_or_default();
        let mut raw_data = tensor.raw_data;
        raw_data.push(self.tag);
        Ok(tonic::Response::new(ExecuteForwardResponse {
            success: true,
            output_activation_tensor: Some(TensorBuffer {
                shape: tensor.shape,
                data_type: tensor.data_type,
                raw_data,
            }),
            execution_time_us: self.execution_time_us,
        }))
    }

    async fn unload_layers(
        &self,
        request: tonic::Request<UnloadLayersRequest>,
    ) -> Result<tonic::Response<UnloadLayersResponse>, tonic::Status> {
        let request = request.into_inner();
        self.record(format!("unload:{}:{}", self.node_id, request.model_id));
        Ok(tonic::Response::new(UnloadLayersResponse {
            success: true,
            error_message: String::new(),
            freed_ram_bytes: 1 << 20,
        }))
    }

    async fn get_health_status(
        &self,
        _request: tonic::Request<HealthRequest>,
    ) -> Result<tonic::Response<HealthResponse>, tonic::Status> {
        Ok(tonic::Response::new(HealthResponse {
            battery_percentage: 80.0,
            cpu_temp_celsius: 40.0,
            free_ram_bytes: 1 << 29,
        }))
    }

    /// Fixed, node-specific "measurements" so a test can assert the exact row
    /// the master recorded came from the wire, not from a derivation.
    async fn benchmark(
        &self,
        _request: tonic::Request<BenchmarkRequest>,
    ) -> Result<tonic::Response<BenchmarkResponse>, tonic::Status> {
        if !self.serve_benchmark {
            return Err(tonic::Status::unimplemented(
                "benchmark probe is not supported by this worker",
            ));
        }
        let tag = f64::from(self.tag);
        Ok(tonic::Response::new(BenchmarkResponse {
            memory_bandwidth_mbs: 1000.0 + tag,
            throughput_flops: 100.0 + tag,
            probe_duration_ms: 4.0 + tag,
        }))
    }
}

/// Serves `worker` over plaintext gRPC on an ephemeral loopback port.
pub async fn serve_mock(worker: MockWorker) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    let incoming = TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        Server::builder()
            .add_service(WorkerServiceServer::new(worker))
            .serve_with_incoming(incoming)
            .await
            .expect("mock worker server");
    });
    (port, handle)
}

/// Serves `worker` behind mutual TLS using `server` as the TLS identity and
/// requiring a client certificate chained to `client_ca_pem`.
///
/// `client_auth_optional(false)` is what makes a completed call evidence that
/// the driver actually presented a client certificate.
pub async fn serve_mock_identity(
    worker: MockWorker,
    server: &ServerIdentity,
    client_ca_pem: &str,
) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    let tls = ServerTlsConfig::new()
        .identity(Identity::from_pem(&server.cert_pem, &server.key_pem))
        .client_ca_root(Certificate::from_pem(client_ca_pem))
        .client_auth_optional(false);
    let incoming = TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        Server::builder()
            .tls_config(tls)
            .expect("server tls config")
            .add_service(WorkerServiceServer::new(worker))
            .serve_with_incoming(incoming)
            .await
            .expect("mock tls worker server");
    });
    (port, handle)
}

/// Serves `worker` behind mutual TLS using the master's own server identity.
pub async fn serve_mock_tls(worker: MockWorker, ca: &NodeCa) -> (u16, tokio::task::JoinHandle<()>) {
    serve_mock_identity(
        worker,
        &ServerIdentity {
            cert_pem: ca.server_cert_pem().to_string(),
            key_pem: ca.server_key_pem().to_string(),
        },
        &ca.ca_pem(),
    )
    .await
}

pub fn assignment(
    node_id: &str,
    port: u16,
    start_layer: usize,
    end_layer: usize,
) -> LayerAssignment {
    LayerAssignment {
        node_id: node_id.into(),
        device_name: format!("{node_id} device"),
        ip_address: "127.0.0.1".into(),
        port,
        start_layer,
        end_layer,
        total_ram_used_bytes: 1 << 28,
        estimated_latency_ms: 1.0,
    }
}

pub fn events(log: &Events) -> Vec<String> {
    log.lock().expect("events lock").clone()
}

/// `LoadedPipeline` is deliberately not `Debug`, so `expect_err` cannot be used
/// on a load result; unwrap the failure explicitly instead.
pub fn expect_load_error(result: Result<LoadedPipeline, String>) -> String {
    match result {
        Ok(_) => panic!("expected the pipeline load to fail"),
        Err(error) => error,
    }
}

pub fn sample_payload() -> TensorPayload {
    TensorPayload {
        session_id: "session-1".into(),
        sequence_index: 7,
        shape: vec![1, 4, 4096],
        data_bytes: vec![0x10, 0x20],
        data_type: ActivationType::Fp16,
    }
}
