//! Host-side distributed split driver tests.
//!
//! Drives [`LoadedPipeline`] — connect → handshake → `LoadLayers` → chained
//! `ExecuteForward` → `UnloadLayers` — against in-process mock workers. The
//! mocks stand in for the not-yet-built native engine: each one appends a
//! unique tag byte to the activation it receives, so the final tensor proves
//! the stages ran in layer order and that the handoff threaded through them.

use ai_upcycler_core::rpc::engine::{ActivationType, LoadedPipeline, TensorPayload};
use ai_upcycler_core::rpc::proto::aiupcycler::rpc::worker_service_server::{
    WorkerService, WorkerServiceServer,
};
use ai_upcycler_core::rpc::proto::aiupcycler::rpc::{
    ExecuteForwardRequest, ExecuteForwardResponse, HandshakeRequest, HandshakeResponse,
    HealthRequest, HealthResponse, LoadLayersRequest, LoadLayersResponse, TensorBuffer,
    UnloadLayersRequest, UnloadLayersResponse,
};
use ai_upcycler_core::scheduler::planner::LayerAssignment;
use std::sync::{Arc, Mutex};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

/// Deterministic stand-in for the native worker engine.
///
/// * `handshake` reports a fixed identity.
/// * `load_layers` records the range; `fail_load` makes it refuse.
/// * `execute_forward` appends `tag` to the incoming activation and reports a
///   fixed latency; `fail_forward` makes it reject.
/// * `unload_layers` records the release.
///
/// Every call is appended to the shared `events` log so a test can assert the
/// exact cross-worker order the driver produced.
struct MockWorker {
    node_id: String,
    tag: u8,
    execution_time_us: i64,
    fail_load: bool,
    fail_forward: bool,
    events: Arc<Mutex<Vec<String>>>,
}

impl MockWorker {
    fn new(
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
            events,
        }
    }

    fn refusing_load(mut self) -> Self {
        self.fail_load = true;
        self
    }

    fn refusing_forward(mut self) -> Self {
        self.fail_forward = true;
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
}

/// Serves `worker` over plaintext gRPC on an ephemeral loopback port.
async fn serve_mock(worker: MockWorker) -> (u16, tokio::task::JoinHandle<()>) {
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

fn assignment(node_id: &str, port: u16, start_layer: usize, end_layer: usize) -> LayerAssignment {
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

fn events(log: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    log.lock().expect("events lock").clone()
}

/// `LoadedPipeline` is deliberately not `Debug`, so `expect_err` cannot be used
/// on a load result; unwrap the failure explicitly instead.
fn expect_load_error(result: Result<LoadedPipeline, String>) -> String {
    match result {
        Ok(_) => panic!("expected the pipeline load to fail"),
        Err(error) => error,
    }
}

fn sample_payload() -> TensorPayload {
    TensorPayload {
        session_id: "session-1".into(),
        sequence_index: 7,
        shape: vec![1, 4, 4096],
        data_bytes: vec![0x10, 0x20],
        data_type: ActivationType::Fp16,
    }
}

#[tokio::test]
async fn loads_runs_and_unloads_in_layer_order_across_workers() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (port_a, server_a) = serve_mock(MockWorker::new("phone-a", 0xA1, 100, log.clone())).await;
    let (port_b, server_b) = serve_mock(MockWorker::new("phone-b", 0xB2, 250, log.clone())).await;

    let plan = [
        assignment("phone-a", port_a, 0, 3),
        assignment("phone-b", port_b, 4, 7),
    ];

    let mut pipeline = LoadedPipeline::load("llama-3-8b", &plan)
        .await
        .expect("load pipeline");
    assert_eq!(pipeline.stage_count(), 2);

    let result = pipeline
        .forward(sample_payload())
        .await
        .expect("forward through both stages");

    // phone-a appends 0xA1, phone-b appends 0xB2: the byte order proves the
    // activation flowed phone-a -> phone-b, not the reverse.
    assert_eq!(result.output_bytes, vec![0x10, 0x20, 0xA1, 0xB2]);
    assert_eq!(result.output_shape, vec![1, 4, 4096]);
    assert_eq!(result.output_data_type, ActivationType::Fp16);
    assert_eq!(result.execution_time_us, 350, "100us + 250us");
    assert_eq!(result.session_id, "session-1");
    assert_eq!(result.sequence_index, 7);

    pipeline.unload().await.expect("unload pipeline");

    assert_eq!(
        events(&log),
        vec![
            "handshake:phone-a",
            "load:phone-a:0..3",
            "handshake:phone-b",
            "load:phone-b:4..7",
            "forward:phone-a",
            "forward:phone-b",
            "unload:phone-a:llama-3-8b",
            "unload:phone-b:llama-3-8b",
        ]
    );

    server_a.abort();
    server_b.abort();
}

#[tokio::test]
async fn establishes_layer_order_from_an_out_of_order_plan() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (port_a, server_a) = serve_mock(MockWorker::new("phone-a", 0xA1, 100, log.clone())).await;
    let (port_b, server_b) = serve_mock(MockWorker::new("phone-b", 0xB2, 250, log.clone())).await;

    // phone-b's range (layers 4..7) is listed first, but the driver must still
    // load and run phone-a (layers 0..3) ahead of it.
    let plan = [
        assignment("phone-b", port_b, 4, 7),
        assignment("phone-a", port_a, 0, 3),
    ];

    let mut pipeline = LoadedPipeline::load("llama-3-8b", &plan)
        .await
        .expect("load pipeline");
    let result = pipeline.forward(sample_payload()).await.expect("forward");
    assert_eq!(
        result.output_bytes,
        vec![0x10, 0x20, 0xA1, 0xB2],
        "phone-a's tag must precede phone-b's regardless of input order"
    );
    pipeline.unload().await.expect("unload");

    let log = events(&log);
    let handshake_a = log
        .iter()
        .position(|event| event == "handshake:phone-a")
        .expect("phone-a handshake recorded");
    let handshake_b = log
        .iter()
        .position(|event| event == "handshake:phone-b")
        .expect("phone-b handshake recorded");
    assert!(
        handshake_a < handshake_b,
        "stages must connect in ascending layer order, log: {log:?}"
    );

    server_a.abort();
    server_b.abort();
}

#[tokio::test]
async fn load_failure_on_a_later_stage_unloads_the_earlier_stages() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (port_a, server_a) = serve_mock(MockWorker::new("phone-a", 0xA1, 100, log.clone())).await;
    let (port_b, server_b) =
        serve_mock(MockWorker::new("phone-b", 0xB2, 250, log.clone()).refusing_load()).await;

    let plan = [
        assignment("phone-a", port_a, 0, 3),
        assignment("phone-b", port_b, 4, 7),
    ];

    let error = expect_load_error(LoadedPipeline::load("llama-3-8b", &plan).await);
    assert!(
        error.contains("phone-b") && error.contains("refused"),
        "error should name the refusing worker, got: {error}"
    );

    let log = events(&log);
    assert!(
        log.contains(&"unload:phone-a:llama-3-8b".to_string()),
        "rollback must release the stage that did load, log: {log:?}"
    );
    assert!(
        !log.iter().any(|event| event.starts_with("forward:")),
        "a failed load must not run any forward pass, log: {log:?}"
    );

    server_a.abort();
    server_b.abort();
}

#[tokio::test]
async fn forward_failure_surfaces_the_worker_error_and_still_unloads() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (port_a, server_a) = serve_mock(MockWorker::new("phone-a", 0xA1, 100, log.clone())).await;
    let (port_b, server_b) =
        serve_mock(MockWorker::new("phone-b", 0xB2, 250, log.clone()).refusing_forward()).await;

    let plan = [
        assignment("phone-a", port_a, 0, 3),
        assignment("phone-b", port_b, 4, 7),
    ];
    let mut pipeline = LoadedPipeline::load("llama-3-8b", &plan)
        .await
        .expect("load pipeline");

    let error = pipeline
        .forward(sample_payload())
        .await
        .expect_err("forward must fail when a worker rejects");
    assert!(
        error.contains("rejected forward step"),
        "error should describe the rejected step, got: {error}"
    );

    pipeline
        .unload()
        .await
        .expect("pipeline stays cleanable after a forward error");

    let log = events(&log);
    assert!(
        log.contains(&"unload:phone-a:llama-3-8b".to_string())
            && log.contains(&"unload:phone-b:llama-3-8b".to_string()),
        "both stages must be released after a failed forward, log: {log:?}"
    );

    server_a.abort();
    server_b.abort();
}

#[tokio::test]
async fn refuses_a_plan_with_no_assignments() {
    let error = expect_load_error(LoadedPipeline::load("llama-3-8b", &[]).await);
    assert!(error.contains("no layer assignments"), "got: {error}");
}

#[tokio::test]
async fn rejects_an_overlapping_plan_before_contacting_any_worker() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (port_a, server_a) = serve_mock(MockWorker::new("phone-a", 0xA1, 100, log.clone())).await;
    let (port_b, server_b) = serve_mock(MockWorker::new("phone-b", 0xB2, 250, log.clone())).await;

    // phone-a owns 0..3 and phone-b claims 2..7: layers 2 and 3 would be
    // executed twice, so the split would produce wrong activations.
    let plan = [
        assignment("phone-a", port_a, 0, 3),
        assignment("phone-b", port_b, 2, 7),
    ];
    let error = expect_load_error(LoadedPipeline::load("llama-3-8b", &plan).await);
    assert!(error.contains("overlap"), "got: {error}");
    assert!(
        events(&log).is_empty(),
        "an invalid plan must be rejected before any worker is dialed"
    );

    server_a.abort();
    server_b.abort();
}

#[tokio::test]
async fn rejects_a_plan_with_a_layer_gap_before_contacting_any_worker() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (port_a, server_a) = serve_mock(MockWorker::new("phone-a", 0xA1, 100, log.clone())).await;
    let (port_b, server_b) = serve_mock(MockWorker::new("phone-b", 0xB2, 250, log.clone())).await;

    // Layers 4 and 5 are owned by nobody: the pipeline would silently skip
    // them and hand the next stage the wrong activation.
    let plan = [
        assignment("phone-a", port_a, 0, 3),
        assignment("phone-b", port_b, 6, 9),
    ];
    let error = expect_load_error(LoadedPipeline::load("llama-3-8b", &plan).await);
    assert!(error.contains("not assigned"), "got: {error}");
    assert!(
        events(&log).is_empty(),
        "an invalid plan must be rejected before any worker is dialed"
    );

    server_a.abort();
    server_b.abort();
}

#[tokio::test]
async fn rejects_a_plan_that_does_not_start_at_layer_zero() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (port_a, server_a) = serve_mock(MockWorker::new("phone-a", 0xA1, 100, log.clone())).await;
    let (port_b, server_b) = serve_mock(MockWorker::new("phone-b", 0xB2, 250, log.clone())).await;

    // A plan partitions the model, so beginning at layer 4 would silently skip
    // the model head — the same silent-wrong-output class as an internal gap.
    let plan = [
        assignment("phone-a", port_a, 4, 7),
        assignment("phone-b", port_b, 8, 11),
    ];
    let error = expect_load_error(LoadedPipeline::load("llama-3-8b", &plan).await);
    assert!(error.contains("layer 0"), "got: {error}");
    assert!(
        events(&log).is_empty(),
        "an invalid plan must be rejected before any worker is dialed"
    );

    server_a.abort();
    server_b.abort();
}

#[tokio::test]
async fn rejects_an_inverted_layer_range() {
    // Port 1 is never bound, so reaching the network at all would surface as a
    // connection error instead of the validation error asserted below.
    let plan = [assignment("phone-a", 1, 5, 2)];
    let error = expect_load_error(LoadedPipeline::load("llama-3-8b", &plan).await);
    assert!(error.contains("inverted"), "got: {error}");
}
