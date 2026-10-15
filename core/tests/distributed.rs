//! Host-side distributed split driver tests.
//!
//! Drives [`LoadedPipeline`] — connect → handshake → `LoadLayers` → chained
//! `ExecuteForward` → `UnloadLayers` — against in-process mock workers. The
//! mocks stand in for the not-yet-built native engine: each one appends a
//! unique tag byte to the activation it receives, so the final tensor proves
//! the stages ran in layer order and that the handoff threaded through them.

mod common;

use ai_upcycler_core::rpc::engine::{
    ActivationType, LoadedPipeline, WorkerTlsConfig, WorkerTransport, WorkerTransportSource,
};
use ai_upcycler_core::security::{ClientIdentity, NodeCa};
use common::{
    assignment, events, expect_load_error, sample_payload, serve_mock, serve_mock_tls, MockWorker,
};
use std::sync::{Arc, Mutex};

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

// ---------------------------------------------------------------------------
// mTLS transport
// ---------------------------------------------------------------------------

/// Test double for the credential store. Only enrolled node ids resolve; any
/// other worker is an error, so an incomplete store cannot silently reach a
/// worker over plaintext.
struct EnrolledTransports {
    ca_pem: String,
    identity: ClientIdentity,
    known: Vec<String>,
}

impl EnrolledTransports {
    /// The driver both trusts and authenticates with `ca`.
    fn new(ca: &NodeCa, known: &[&str]) -> Self {
        Self::pinning(ca, ca, known)
    }

    /// `pin` is the CA whose signature the driver requires on the worker's
    /// server certificate; `identity_ca` issues the client certificate the
    /// driver presents. Splitting the two lets a test isolate server-certificate
    /// pinning from the server's own acceptance of the client certificate.
    fn pinning(pin: &NodeCa, identity_ca: &NodeCa, known: &[&str]) -> Self {
        Self {
            ca_pem: pin.ca_pem(),
            identity: identity_ca
                .issue_client_cert("master")
                .expect("client identity"),
            known: known.iter().map(|id| id.to_string()).collect(),
        }
    }
}

impl WorkerTransportSource for EnrolledTransports {
    fn transport_for(&self, node_id: &str) -> Result<WorkerTransport, String> {
        if !self.known.iter().any(|known| known == node_id) {
            return Err(format!("no enrolled credentials for worker {node_id}"));
        }
        Ok(WorkerTransport::MutualTls(WorkerTlsConfig {
            ca_pem: self.ca_pem.clone(),
            client_cert_pem: self.identity.cert_pem.clone(),
            client_key_pem: self.identity.key_pem.clone(),
        }))
    }
}

#[tokio::test]
async fn loads_runs_and_unloads_over_mtls() {
    let ca = NodeCa::generate().expect("ca");
    let log = Arc::new(Mutex::new(Vec::new()));
    let (port_a, server_a) =
        serve_mock_tls(MockWorker::new("phone-a", 0xA1, 100, log.clone()), &ca).await;
    let (port_b, server_b) =
        serve_mock_tls(MockWorker::new("phone-b", 0xB2, 250, log.clone()), &ca).await;

    let plan = [
        assignment("phone-a", port_a, 0, 3),
        assignment("phone-b", port_b, 4, 7),
    ];
    let transports = EnrolledTransports::new(&ca, &["phone-a", "phone-b"]);

    let mut pipeline = LoadedPipeline::load_secure("llama-3-8b", &plan, &transports)
        .await
        .expect("load over mTLS");
    assert_eq!(pipeline.stage_count(), 2);

    let result = pipeline
        .forward(sample_payload())
        .await
        .expect("forward over mTLS");
    assert_eq!(result.output_bytes, vec![0x10, 0x20, 0xA1, 0xB2]);
    assert_eq!(result.output_shape, vec![1, 4, 4096]);
    assert_eq!(result.output_data_type, ActivationType::Fp16);
    assert_eq!(result.execution_time_us, 350, "100us + 250us");

    pipeline.unload().await.expect("unload over mTLS");

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

/// The worker's certificate is pinned to the cluster CA from the out-of-band
/// pairing payload, so a worker presenting a certificate from any other CA must
/// be refused. Here the impostor happily accepts our client certificate (it is
/// signed by the same impostor CA), so the driver's pinning is the only thing
/// that can reject it.
#[tokio::test]
async fn refuses_a_worker_presenting_a_certificate_from_an_untrusted_ca() {
    let trusted_ca = NodeCa::generate().expect("trusted ca");
    let impostor_ca = NodeCa::generate().expect("impostor ca");
    let log = Arc::new(Mutex::new(Vec::new()));

    let (port, server) = serve_mock_tls(
        MockWorker::new("phone-a", 0xA1, 100, log.clone()),
        &impostor_ca,
    )
    .await;
    let transports = EnrolledTransports::pinning(&trusted_ca, &impostor_ca, &["phone-a"]);
    let plan = [assignment("phone-a", port, 0, 3)];

    let error =
        expect_load_error(LoadedPipeline::load_secure("llama-3-8b", &plan, &transports).await);
    assert!(!error.is_empty(), "a pinned mismatch must be an error");
    assert!(
        events(&log).iter().all(|event| !event.starts_with("load:")),
        "a rejected TLS handshake must not load layers, log: {:?}",
        events(&log)
    );

    server.abort();
}

/// Plan validation is not the only thing that must fail before dialing: so must
/// an incomplete credential store, so a worker is never contacted with the wrong
/// (or a missing) identity.
#[tokio::test]
async fn refuses_to_dial_a_worker_with_no_enrolled_credentials() {
    let ca = NodeCa::generate().expect("ca");
    let log = Arc::new(Mutex::new(Vec::new()));
    let (port_a, server_a) =
        serve_mock_tls(MockWorker::new("phone-a", 0xA1, 100, log.clone()), &ca).await;
    let (port_b, server_b) =
        serve_mock_tls(MockWorker::new("phone-b", 0xB2, 250, log.clone()), &ca).await;

    let plan = [
        assignment("phone-a", port_a, 0, 3),
        assignment("phone-b", port_b, 4, 7),
    ];
    // Only phone-a is enrolled. phone-b is the *later* stage, so an
    // implementation that resolved transports lazily, stage by stage, would
    // already have contacted phone-a before noticing.
    let transports = EnrolledTransports::new(&ca, &["phone-a"]);

    let error =
        expect_load_error(LoadedPipeline::load_secure("llama-3-8b", &plan, &transports).await);
    assert!(
        error.contains("phone-b"),
        "error should name the unenrolled worker, got: {error}"
    );
    assert!(
        events(&log).is_empty(),
        "a missing credential must fail before any worker is dialed, log: {:?}",
        events(&log)
    );

    server_a.abort();
    server_b.abort();
}

/// A worker the store reports as TLS must never be reached over plaintext. The
/// listener here is plaintext, so a silent fallback would succeed; the driver
/// has no fallback path, so the load must fail instead.
#[tokio::test]
async fn never_downgrades_a_tls_worker_to_plaintext() {
    let ca = NodeCa::generate().expect("ca");
    let log = Arc::new(Mutex::new(Vec::new()));
    let (port, server) = serve_mock(MockWorker::new("phone-a", 0xA1, 100, log.clone())).await;
    let transports = EnrolledTransports::new(&ca, &["phone-a"]);
    let plan = [assignment("phone-a", port, 0, 3)];

    let error =
        expect_load_error(LoadedPipeline::load_secure("llama-3-8b", &plan, &transports).await);
    assert!(!error.is_empty(), "plaintext must not satisfy a TLS worker");
    assert!(
        events(&log).is_empty(),
        "no plaintext fallback is permitted, log: {:?}",
        events(&log)
    );

    server.abort();
}
