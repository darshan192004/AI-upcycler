//! Model push end to end: a host [`ModelSource`] streamed to a mock worker
//! (spec `2026-11-03-model-provisioning`).

mod common;

use ai_upcycler_core::provisioning::{ModelSource, CHUNK_BYTES};
use ai_upcycler_core::rpc::client::WorkerRpcClient;
use common::{serve_mock, Events, MockWorker};
use futures::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ai-upcycler-provisioning-e2e-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write");
    path
}

/// Enough bytes to need four chunks, with a tail that is not a whole chunk.
fn payload() -> Vec<u8> {
    (0..=255u8).cycle().take(3 * CHUNK_BYTES + 777).collect()
}

async fn dial(port: u16) -> WorkerRpcClient {
    WorkerRpcClient::connect(&format!("http://127.0.0.1:{port}"))
        .await
        .expect("connect")
}

#[tokio::test]
async fn a_full_push_delivers_every_byte() {
    let dir = temp_dir();
    let bytes = payload();
    let path = write(&dir, "m.gguf", &bytes);
    let source = ModelSource::open(&path).expect("open");
    let worker = MockWorker::new("phone", 1, 10, Events::default());
    let push = worker.push_log();
    let (port, _server) = serve_mock(worker).await;

    let mut client = dial(port).await;
    let reply = client
        .provision_model(&source, "m")
        .await
        .expect("provision");

    assert!(reply.success, "{}", reply.error_message);
    assert_eq!(bytes.len() as i64, reply.size_bytes);
    assert_eq!(bytes, push.lock().expect("push lock").bytes);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_push_resumes_at_the_workers_offset() {
    let dir = temp_dir();
    let bytes = payload();
    let path = write(&dir, "m.gguf", &bytes);
    let source = ModelSource::open(&path).expect("open");
    let worker = MockWorker::new("phone", 1, 10, Events::default());
    let push = worker.push_log();
    // The worker already holds a prefix, as after a dropped connection.
    push.lock()
        .expect("push lock")
        .bytes
        .extend_from_slice(&bytes[..1_000_000]);
    let (port, _server) = serve_mock(worker).await;

    let mut client = dial(port).await;
    // Starting at 0 against a worker that holds 1,000,000 bytes would be
    // refused as an offset mismatch, so a successful push proves the resume.
    let reply = client
        .provision_model(&source, "m")
        .await
        .expect("provision");

    assert!(reply.success, "{}", reply.error_message);
    assert_eq!(bytes, push.lock().expect("push lock").bytes);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_rejected_begin_makes_no_further_calls() {
    let dir = temp_dir();
    let bytes = payload();
    let path = write(&dir, "m.gguf", &bytes);
    let source = ModelSource::open(&path).expect("open");
    let events: Events = Arc::default();
    let worker = MockWorker::new("phone", 1, 10, Arc::clone(&events));
    let push = worker.push_log();
    push.lock().expect("push lock").refusal = Some("not enough free space".into());
    let (port, _server) = serve_mock(worker).await;

    let mut client = dial(port).await;
    let error = client
        .provision_model(&source, "m")
        .await
        .expect_err("a refused begin must stop the push");

    assert!(error.contains("not enough free space"), "{error}");
    assert!(push.lock().expect("push lock").bytes.is_empty());
    assert!(
        events.lock().expect("events lock").is_empty(),
        "no RPC after a refused begin: {:?}",
        events.lock().expect("events lock")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn chunk_stream_reassembles_the_file() {
    let dir = temp_dir();
    let bytes = payload();
    let path = write(&dir, "m.gguf", &bytes);
    let source = ModelSource::open(&path).expect("open");

    let mut stream = Box::pin(source.chunk_stream(0));
    let mut out = Vec::new();
    let mut chunks = 0usize;
    while let Some(chunk) = stream.next().await {
        assert_eq!(out.len() as i64, chunk.offset, "chunks must be contiguous");
        out.extend_from_slice(&chunk.data);
        chunks += 1;
    }

    assert_eq!(bytes, out);
    assert!(chunks >= 4, "expected four chunks, got {chunks}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn chunk_stream_stops_early_when_the_file_disappears() {
    let dir = temp_dir();
    let bytes = payload();
    let path = write(&dir, "m.gguf", &bytes);
    let source = ModelSource::open(&path).expect("open");

    let mut stream = Box::pin(source.chunk_stream(0));
    let first = stream.next().await.expect("first chunk");
    assert_eq!(0, first.offset);
    // The master's model was deleted underneath the transfer: rather than
    // emit a wrong byte, the stream ends and the worker's finish then
    // reports the file as incomplete.
    std::fs::remove_file(&path).expect("delete");
    assert!(stream.next().await.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
