//! Shared helpers for the worker integration tests: process-spawn and RPC
//! client plumbing around the engine, server and client types this crate
//! owns. The deterministic GGUF fixture writer they all stand on is
//! `ai_upcycler_model_io::fixture` (re-homed there in the model-io
//! extraction); the old names keep working through the re-export below.

#![allow(dead_code)] // helpers shared across several worker test suites

use std::io::BufRead;
use std::path::Path;

pub use ai_upcycler_model_io::fixture::*;

/// Reads a spawned worker's stdout until the `READY <ip:port>` line.
pub fn read_ready_line(out: &mut impl std::io::Read) -> String {
    let reader = std::io::BufReader::new(out);
    for line in reader.lines() {
        let line = line.expect("read stdout line");
        if let Some(addr) = line.strip_prefix("READY ") {
            return addr.trim().to_string();
        }
    }
    panic!("worker exited before printing READY");
}

/// Spawns the worker **binary** on an ephemeral loopback port and returns
/// (port, child) once the `READY` line arrives. The caller owns the child:
/// kill + wait it when done.
pub fn spawn_worker(model_path: &Path, worker_id: &str) -> (u16, std::process::Child) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ai-upcycler-worker"))
        .args([
            "--model",
            model_path.to_str().expect("model path is utf8"),
            "--bind",
            "127.0.0.1:0",
            "--worker-id",
            worker_id,
        ])
        .stdout(std::process::Stdio::piped())
        // stderr inherited on purpose: the engine explains forward refusals
        // on stderr, and a refused step must show WHY in the test log.
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("spawn worker binary");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let addr = read_ready_line(&mut stdout);
    let port: u16 = addr
        .rsplit(':')
        .next()
        .expect("READY carries an ip:port")
        .parse()
        .expect("READY port is numeric");
    (port, child)
}

/// Stage-0 activation payload for the engine's `[1, n]` contract: F32 values
/// holding the token ids (the engine decodes `raw_data` as f32 and validates
/// each against the vocabulary), at sequence position 0.
pub fn ids_payload(session: &str, ids: &[u32]) -> ai_upcycler_core::rpc::engine::TensorPayload {
    let mut data = Vec::with_capacity(ids.len() * 4);
    for id in ids {
        data.extend_from_slice(&(*id as f32).to_le_bytes());
    }
    ai_upcycler_core::rpc::engine::TensorPayload {
        session_id: session.to_string(),
        sequence_index: 0,
        shape: vec![1, ids.len() as i64],
        data_bytes: data,
        data_type: ai_upcycler_core::rpc::engine::ActivationType::Fp32,
    }
}

/// Serves an engine in-process on a loopback port (mirrors core's
/// `serve_mock`); returns the port and the running server task.
pub async fn serve_engine(
    engine: ai_upcycler_worker::engine::WorkerEngine,
) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let port = listener.local_addr().expect("local addr").port();
    let handle = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(
                ai_upcycler_worker::proto::aiupcycler::rpc::worker_service_server::WorkerServiceServer::new(
                    engine,
                ),
            )
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .expect("engine server");
    });
    (port, handle)
}

/// Serves `fx`'s engine in-process and returns a connected client.
pub async fn connect_engine(
    fx: &Fixture,
) -> ai_upcycler_worker::proto::aiupcycler::rpc::worker_service_client::WorkerServiceClient<
    tonic::transport::Channel,
> {
    let engine =
        ai_upcycler_worker::engine::WorkerEngine::new(&fx.model_id, "test-device", fx.path.clone())
            .expect("engine parses fixture");
    let (port, _server) = serve_engine(engine).await;
    ai_upcycler_worker::proto::aiupcycler::rpc::worker_service_client::WorkerServiceClient::connect(
        format!("http://127.0.0.1:{port}"),
    )
    .await
    .expect("connect engine client")
}
