//! Cluster-backed chat serving vs `llama-server` (epic PR-1 acceptance gate,
//! design D2/D3): the REAL axum router serves `/v1/chat/completions` through
//! ONE local `ai-upcycler-worker` subprocess (full model, single stage,
//! loopback plaintext transport). With `temperature: 0` both paths are
//! greedy, so the streamed master text must be string-equal to the host
//! server's continuation for the same prompt. The reference runs llama's RAW
//! `/completion` endpoint with the hand-rendered ChatML prompt: this build's
//! `/v1/chat/completions` prepends the rendered prompt to `content`, which
//! corrupts a continuation-vs-continuation comparison.
//!
//! Measured parity (2026-10-18, release profile): llama picks id 59604
//! "Paris" (top logprob -0.123) then id 151645 `<|im_end|>`; the master
//! sampled the same ids id-for-id, back-to-back runs byte-identical (the
//! forward path is a pure function of the prompt).
//!
//! Asset-gated like `model-io/tests/tokenizer_parity.rs`; skips (with a
//! printed message) unless ALL of:
//! - `AI_UPCCLER_MODEL` - the real `.gguf` (Qwen2.5-0.5B class)
//! - `AI_UPCCLER_LLAMA_SERVER` - path to the `llama-server` binary
//! - `AI_UPCCLER_WORKER_BIN` - path to a built `ai-upcycler-worker` binary
//!   (`cargo build -p ai-upcycler-worker`; the core test crate cannot build
//!   the worker package itself - a core dev-dependency on the worker would
//!   be a dependency cycle). IMPORTANT: run with `--release` - measured
//!   (2026-10-18) per-forward cost in a DEBUG build exceeds the RPC
//!   deadline for the real model, while the release profile passes
//!   (the same bisect raised `REQUEST_TIMEOUT` to 120s).
//!
//! Independent local checks build fine without any of these (the file
//! compiles by using only public APIs).
//!
//! Greedy caveat recorded: our logits differ from llama.cpp's by the
//! quantized-kernel kernel gap measured in `worker/tests/parity.rs`
//! (worst |logit delta| ~0.17 nats), so a near-tie continuation MAY differ.
//! The chosen prompts are ones where `parity.rs` and manual runs agree; a
//! near-tie divergence is a legitimate failure that must be investigated
//! against that gap, not papered over.
//!
//! The mock API on :8080 is untouched by this file; servers are spawned on
//! ephemeral loopback ports and reaped by guards.

use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::distributed::AdaptiveBackend;
use ai_upcycler_core::api::AppState;
use ai_upcycler_core::discovery::mdns::{DiscoveredNode, NodeRegistry};
use ai_upcycler_core::rpc::engine::{WorkerTransport, WorkerTransportSource};
use ai_upcycler_core::security::SecurityState;
use ai_upcycler_core::store::{NewModel, Store};
use ai_upcycler_core::telemetry::TelemetryBus;

/// Kills every spawned child no matter how the test exits.
struct ChildGuard(Vec<Child>);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        for child in self.0.iter_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn env_or_skip(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() && std::path::Path::new(&value).exists() => Some(value),
        _ => {
            eprintln!(
                "skipping: set {name} (plus AI_UPCCLER_MODEL, AI_UPCCLER_LLAMA_SERVER, \
                 AI_UPCCLER_WORKER_BIN) to run the real-router serving gate"
            );
            None
        }
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("addr")
        .port()
}

fn http_get(port: u16, path: &str) -> Result<String, String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).expect("write request");
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).expect("read response");
    Ok(String::from_utf8_lossy(&buf).replace('\u{0}', ""))
}

fn http_post(port: u16, path: &str, body: &str, timeout_secs: u64) -> (String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
    s.set_read_timeout(Some(Duration::from_secs(timeout_secs)))
        .ok();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).expect("write head");
    s.write_all(body.as_bytes()).expect("write body");
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).expect("read response");
    let text = String::from_utf8_lossy(&buf).replace('\u{0}', "");
    let status = text.lines().next().unwrap_or("").to_string();
    let body = match text.split_once("\r\n\r\n") {
        Some((_, b)) => b.replace("data: [DONE]", "").to_string(),
        None => String::new(),
    };
    (status, body)
}

/// Minimal JSON string unescape for `content` fields.
fn json_unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(code) = u32::from_str_radix(&hex, 16) {
                    if let Some(ch) = char::from_u32(code) {
                        out.push(ch);
                    }
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Pulls every `"content":"..."` delta out of an SSE chunk stream body.
fn sse_contents(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let marker = "\"content\":\"";
        let Some(start) = data.find(marker) else {
            continue;
        };
        let rest = &data[start + marker.len()..];
        // The delta is the longest run without an unescaped quote: good
        // enough for the prompts this test uses (no escaped quotes in the
        // model's answer apart from the common cases handled above).
        let mut end = rest.len();
        for (index, ch) in rest.char_indices() {
            if ch == '"' && !rest[..index].ends_with('\\') {
                end = index;
                break;
            }
        }
        out.push(json_unescape(&rest[..end]));
    }
    out
}

/// The `content` string of a single non-streaming completion response.
fn buffered_content(body: &str) -> Option<String> {
    let marker = "\"content\":\"";
    let start = body.find(marker)?;
    let rest = &body[start + marker.len()..];
    let mut end = rest.len();
    for (index, ch) in rest.char_indices() {
        if ch == '"' && !rest[..index].ends_with('\\') {
            end = index;
            break;
        }
    }
    Some(json_unescape(&rest[..end]))
}

/// Loopback plaintext transport source: the TEST-ONLY stand-in for the
/// pairing-backed source production uses (design D3 - test vehicle, never a
/// production path).
struct LoopbackPlaintexts;

impl WorkerTransportSource for LoopbackPlaintexts {
    fn transport_for(&self, _node_id: &str) -> Result<WorkerTransport, String> {
        Ok(WorkerTransport::Plaintext)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "asset-gated: needs AI_UPCCLER_MODEL + AI_UPCCLER_LLAMA_SERVER + AI_UPCCLER_WORKER_BIN"]
async fn real_router_serves_greedy_chat_through_one_worker_parity_with_llama() {
    let Some(model) = env_or_skip("AI_UPCCLER_MODEL") else {
        return;
    };
    let Some(server_bin) = env_or_skip("AI_UPCCLER_LLAMA_SERVER") else {
        return;
    };
    let Some(worker_bin) = env_or_skip("AI_UPCCLER_WORKER_BIN") else {
        return;
    };

    // ---- env sanity: the model must be the Qwen2 arch the tokenizer reads
    let weight_bytes = std::fs::metadata(&model).expect("model metadata").len();
    assert!(
        weight_bytes > 100_000_000,
        "AI_UPCCLER_MODEL must point at the real ~350MB GGUF, got {weight_bytes} bytes"
    );

    // ---- llama-server reference (greedy under temperature 0) --------------
    let llama_port = free_port();
    let mut children = ChildGuard(Vec::new());
    children.0.push(
        Command::new(&server_bin)
            .args([
                "--model",
                &model,
                "--port",
                &llama_port.to_string(),
                "--ctx-size",
                "512",
                "--host",
                "127.0.0.1",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn llama-server"),
    );
    let mut llama_ready = false;
    for _ in 0..120 {
        match http_get(llama_port, "/health") {
            Ok(body) if body.contains("\"status\":\"ok\"") => {
                llama_ready = true;
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    assert!(llama_ready, "llama-server never became healthy");

    // ---- one local worker subprocess holding the whole model --------------
    // `--bind 127.0.0.1:0`: the worker owns its own ephemeral port and
    // publishes it in READY (grabbed ports can be re-assigned between our
    // probe listener closing and the worker binding).
    let mut worker = Command::new(&worker_bin)
        .args([
            "--model",
            &model,
            "--bind",
            "127.0.0.1:0",
            "--worker-id",
            "test-local-phone",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn ai-upcycler-worker");
    let mut reader = std::io::BufReader::new(worker.stdout.take().expect("piped stdout"));
    let mut worker_port_selection: Option<u16> = None;
    let mut ready = false;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if let Some(addr) = line.trim().strip_prefix("READY ") {
            let port_part = addr.rsplit(':').next().expect("READY ip:port");
            worker_port_selection = port_part.parse::<u16>().ok();
            ready = true;
            break;
        }
    }
    let worker_port = worker_port_selection.expect("worker READY carries a parseable ip:port");
    assert!(ready, "worker never printed READY");
    // Drain stdout past READY in the background: an untouched pipe would
    // eventually block the worker's event loop once its 64KB buffer fills.
    std::thread::spawn(move || {
        let mut sink = String::new();
        loop {
            sink.clear();
            if reader.read_line(&mut sink).unwrap_or(0) == 0 {
                break;
            }
        }
    });
    children.0.push(worker);

    // ---- the real AppState: registry node + enrollment + store row --------
    let registry = Arc::new(NodeRegistry::new());
    registry.register_or_update(DiscoveredNode {
        node_id: "test-local-phone".into(),
        device_name: "local test worker".into(),
        ip_address: "127.0.0.1".into(),
        port: worker_port,
        total_ram_bytes: 1 << 34,
        free_ram_bytes: 1 << 33,
        connection_type: "WIFI_MDNS".into(),
        latency_ms: 1.0,
        last_seen_epoch_ms: 0,
    });

    let security = Arc::new(SecurityState::default());
    security
        .enrollments
        .enroll("test-local-phone", "local test worker", vec![]);

    let store = Arc::new(Store::open_in_memory().expect("in-memory store"));
    store
        .upsert_model(&NewModel {
            model_id: "qwen2.5-0.5b-instruct-q4_k_m".into(),
            file_name: "qwen2.5-0.5b-instruct-q4_k_m.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 24,
            weight_size_bytes: weight_bytes as i64,
            local_file_path: model.clone(),
        })
        .expect("store row");

    let prompt = "The capital of France is";
    let max_tokens: u32 = 8;

    let adaptive = AdaptiveBackend::new(
        registry.clone(),
        store.clone(),
        security,
        Arc::new(StubBackend::new()),
    )
    .with_transports(Arc::new(LoopbackPlaintexts));

    let state = AppState::new(
        "serving-test".to_string(),
        registry,
        store,
        Arc::new(adaptive) as Arc<dyn ai_upcycler_core::api::backend::GenerationBackend>,
        TelemetryBus::new(64),
        ai_upcycler_core::api::HostInfo::default(),
    );

    let app_port = free_port();
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{app_port}"))
        .await
        .expect("bind app port");
    tokio::spawn(async move {
        axum::serve(listener, ai_upcycler_core::api::router(state))
            .await
            .expect("serve router");
    });

    // ---- reference: llama-server greedy -----------------------------------
    // Raw /completion (not /v1/chat/completions): the chat endpoint of this
    // build prepends the rendered prompt to `content`, which corrupts the
    // comparison; /completion returns ONLY the continuation.
    let reference = format!(
        "{{\"prompt\":\"<|im_start|>user\\n{prompt}<|im_end|>\\n<|im_start|>assistant\\n\",\"temperature\":0,\"n_predict\":{max_tokens},\"logprobs\":true}}",
        prompt = prompt
    );
    let (status, body) = http_post(llama_port, "/completion", &reference, 120);
    assert!(
        status.contains("200"),
        "llama-server status: {status} {body}"
    );
    let expected = buffered_content(&body).expect("llama content parse");
    assert!(
        !expected.trim().is_empty(),
        "llama-server returned an empty greedy continuation"
    );
    eprintln!("llama-server greedy continuation: {expected:?}");

    // ---- candidate: the REAL router through the cluster backend -----------
    let request_body = format!(
        "{{\"model\":\"qwen2.5-0.5b-instruct-q4_k_m\",\"messages\":[{{\"role\":\"user\",\"content\":\"{}\"}}],\"temperature\":0,\"max_tokens\":{max_tokens},\"stream\":true,\"session_id\":\"parity-session\"}}",
        prompt
    );
    let (status, sse_body) = http_post(app_port, "/v1/chat/completions", &request_body, 300);
    assert!(status.contains("200"), "router status: {status} {sse_body}");
    let streamed: String = sse_contents(&sse_body).concat();
    eprintln!("cluster streamed continuation: {streamed:?}");

    assert!(
        !streamed.trim().is_empty(),
        "cluster backend streamed nothing: {sse_body}"
    );
    assert_eq!(
        streamed, expected,
        "greedy continuations diverge (see the quantized-kernel gap note in the header)"
    );
}
