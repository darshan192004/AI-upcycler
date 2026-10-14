//! End-to-end mTLS handshake tests: PIN-enrolled worker credentials against a
//! CA-enforcing gRPC server that stands in for a real Android worker.

use ai_upcycler_core::rpc::client::{WorkerRpcClient, MASTER_ID, PROTOCOL_VERSION};
use ai_upcycler_core::rpc::proto::aiupcycler::rpc::worker_service_server::{
    WorkerService, WorkerServiceServer,
};
use ai_upcycler_core::rpc::proto::aiupcycler::rpc::{
    ExecuteForwardRequest, ExecuteForwardResponse, HandshakeRequest, HandshakeResponse,
    HealthRequest, HealthResponse, LoadLayersRequest, LoadLayersResponse, UnloadLayersRequest,
    UnloadLayersResponse,
};
use ai_upcycler_core::security::{ClientIdentity, NodeCa, PairingManager, PAIRING_TTL};
use std::net::SocketAddr;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};

/// Test double for the Android worker's `WorkerService`.
#[derive(Default)]
struct TestWorker;

#[tonic::async_trait]
impl WorkerService for TestWorker {
    async fn handshake(
        &self,
        _request: tonic::Request<HandshakeRequest>,
    ) -> Result<tonic::Response<HandshakeResponse>, tonic::Status> {
        Ok(tonic::Response::new(HandshakeResponse {
            worker_id: "test-worker".into(),
            device_name: "Test Worker".into(),
            total_ram_bytes: 1 << 30,
            free_ram_bytes: 1 << 29,
            cpu_cores: 4,
            has_npu_acceleration: false,
        }))
    }

    async fn load_layers(
        &self,
        _request: tonic::Request<LoadLayersRequest>,
    ) -> Result<tonic::Response<LoadLayersResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not needed for mTLS test"))
    }

    async fn execute_forward(
        &self,
        _request: tonic::Request<ExecuteForwardRequest>,
    ) -> Result<tonic::Response<ExecuteForwardResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not needed for mTLS test"))
    }

    async fn unload_layers(
        &self,
        _request: tonic::Request<UnloadLayersRequest>,
    ) -> Result<tonic::Response<UnloadLayersResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not needed for mTLS test"))
    }

    async fn get_health_status(
        &self,
        _request: tonic::Request<HealthRequest>,
    ) -> Result<tonic::Response<HealthResponse>, tonic::Status> {
        Ok(tonic::Response::new(HealthResponse {
            battery_percentage: 99.0,
            cpu_temp_celsius: 35.0,
            free_ram_bytes: 1 << 29,
        }))
    }
}

/// Spawns `TestWorker` behind mTLS: server identity from `ca`, client certs
/// required and validated against `ca` as the only trust anchor.
async fn serve_test_worker(ca: &NodeCa) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let tls = ServerTlsConfig::new()
        .identity(Identity::from_pem(
            ca.server_cert_pem(),
            ca.server_key_pem(),
        ))
        .client_ca_root(Certificate::from_pem(ca.ca_pem()))
        .client_auth_optional(false);
    let incoming = TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        Server::builder()
            .tls_config(tls)
            .expect("server tls config")
            .add_service(WorkerServiceServer::new(TestWorker))
            .serve_with_incoming(incoming)
            .await
            .expect("mTLS test server");
    });
    (addr, handle)
}

/// Full pairing flow: create session → redeem PIN → issue client cert.
fn enroll_via_pin(ca: &NodeCa) -> ClientIdentity {
    let pairing = PairingManager::new(PAIRING_TTL);
    let (pin, qr, _) = pairing.create_session(ca.fingerprint());
    assert!(qr.contains(ca.fingerprint()), "QR carries CA fingerprint");
    assert!(qr.contains(&pin), "QR carries pairing PIN");
    pairing.redeem(&pin).expect("PIN redeems");
    ca.issue_client_cert("test-device").expect("client cert")
}

#[tokio::test]
async fn enrolled_client_completes_mtls_handshake() {
    let ca = NodeCa::generate().expect("ca");
    let identity = enroll_via_pin(&ca);
    let (addr, server) = serve_test_worker(&ca).await;

    let mut client = WorkerRpcClient::connect_tls(
        &format!("https://localhost:{}", addr.port()),
        &ca.ca_pem(),
        &identity.cert_pem,
        &identity.key_pem,
        "localhost",
    )
    .await
    .expect("mTLS connect");

    let response = client
        .handshake(MASTER_ID, PROTOCOL_VERSION)
        .await
        .expect("handshake over mTLS");
    assert_eq!(response.worker_id, "test-worker");
    assert_eq!(response.cpu_cores, 4);

    let health = client.health().await.expect("health over mTLS");
    assert_eq!(health.battery_percentage, 99.0);

    server.abort();
}

#[tokio::test]
async fn client_from_untrusted_ca_is_rejected() {
    let server_ca = NodeCa::generate().expect("server ca");
    let impostor_ca = NodeCa::generate().expect("impostor ca");
    let impostor = impostor_ca.issue_client_cert("impostor").expect("issue");

    let (addr, server) = serve_test_worker(&server_ca).await;

    let result = WorkerRpcClient::connect_tls(
        &format!("https://localhost:{}", addr.port()),
        &server_ca.ca_pem(),
        &impostor.cert_pem,
        &impostor.key_pem,
        "localhost",
    )
    .await;
    // TLS 1.3 client-cert verification resolves on the server after the
    // client's Finished, so the client's `connect` can succeed; the server's
    // verdict surfaces on the first RPC.
    if let Ok(mut client) = result {
        let handshake = client.handshake(MASTER_ID, PROTOCOL_VERSION).await;
        assert!(
            handshake.is_err(),
            "server must reject untrusted client cert, handshake got: {handshake:?}"
        );
    }

    server.abort();
}

#[tokio::test]
async fn client_without_identity_is_rejected() {
    let ca = NodeCa::generate().expect("ca");
    let (addr, server) = serve_test_worker(&ca).await;

    let result = WorkerRpcClient::connect_tls(
        &format!("https://localhost:{}", addr.port()),
        &ca.ca_pem(),
        "",
        "",
        "localhost",
    )
    .await;
    assert!(result.is_err(), "server requires a client certificate");

    server.abort();
}
