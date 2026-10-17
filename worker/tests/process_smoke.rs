//! Process-level smoke test: the worker binary parses args, binds an
//! ephemeral loopback port, prints `READY <addr>`, and answers Handshake and
//! GetHealthStatus over real gRPC.

use ai_upcycler_worker::proto::aiupcycler::rpc::worker_service_client::WorkerServiceClient;
use ai_upcycler_worker::proto::aiupcycler::rpc::{HandshakeRequest, HealthRequest};
use std::process::{Command, Stdio};

mod common;

#[test]
fn worker_binary_serves_handshake_and_health() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());

    let mut child = Command::new(env!("CARGO_BIN_EXE_ai-upcycler-worker"))
        .args([
            "--model",
            fx.path.to_str().unwrap(),
            "--bind",
            "127.0.0.1:0",
            "--worker-id",
            "smoke-1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn worker");

    let mut stdout = child.stdout.take().unwrap();
    let addr = common::read_ready_line(&mut stdout);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut client = rt
        .block_on(async { WorkerServiceClient::connect(format!("http://{addr}")).await })
        .expect("connect to worker");

    let handshake = rt
        .block_on(client.handshake(HandshakeRequest {
            master_id: "smoke-master".into(),
            protocol_version: "1.0".into(),
            timestamp: 0,
        }))
        .expect("handshake")
        .into_inner();
    assert_eq!(handshake.worker_id, "smoke-1");
    assert!(handshake.total_ram_bytes > 0, "real total RAM reported");
    assert!(handshake.free_ram_bytes > 0, "real free RAM reported");
    assert!(handshake.cpu_cores >= 1);

    let health = rt
        .block_on(client.get_health_status(HealthRequest {}))
        .expect("health")
        .into_inner();
    assert!(health.free_ram_bytes > 0);

    child.kill().expect("kill worker");
    let _ = child.wait().expect("reap worker");
}
