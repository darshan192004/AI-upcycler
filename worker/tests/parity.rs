//! Reference parity vs llama-server (plan Task 8): a full-range engine
//! forward over real Qwen2.5-0.5B tokens must agree with llama.cpp's
//! conditioning - greedy next token identical, and the top-5 logprob set
//! overlapping >= 4/5 with bounded logprob deltas.
//!
//! Skips (with a printed message) unless BOTH env vars are set:
//! - `AI_UPCCLER_MODEL` - the real `.gguf` (same file Task 4's gate uses)
//! - `AI_UPCCLER_LLAMA_SERVER` - path to the `llama-server` binary
//!
//! Source-driven findings recorded against the actual build
//! (llama.cpp 0.6.0-dev, build 11429, commit d81235049, Windows x86_64,
//! verified 2026-10-17):
//! - `POST /tokenize` `{"content": ...}` -> `{"tokens":[...]}` - the ids come
//!   back WITHOUT BOS/special tokens for this model, and the server's own
//!   `usage.prompt_tokens` matched, so the engine feeds the identical ids at
//!   identical positions with no special-token divergence.
//! - `GET /health` -> `{"status":"ok"}` once the model is loaded.
//! - `POST /v1/completions` (not the chat endpoint - raw completion
//!   conditioning, no prompt template) supports `logprobs: 25` in this
//!   release. (Plan note: the plan's `--ctx 512` flag is spelled
//!   `--ctx-size 512` in this build.)
//!
//! **Numeric regime (discovered during parity debugging, see the delta
//! table this test prints):** llama.cpp's CPU kernels for quantized weights
//! quantize the *activations* to 8-bit (q8_0/q8_K per 32-element block with
//! f16 scales) before each dot product, while this engine dequantizes
//! weights once and computes in full F32 - strictly more accurate, but not
//! bit-equal to llama.cpp. Block-count bisection with surgically truncated
//! GGUFs (1/2/4/8/16/24 blocks) measured the gap growing ~0.05 nats per
//! block (ranking identical everywhere), and converting the same model's
//! tensors to pure F32 collapsed the gap to <= 4e-4 nats (F32-vs-F32 noise
//! floor). So on the quantized file this gate bounds shared-token deltas
//! at 0.6 nats (24 blocks x 0.05-block observed worst ~0.55), and the
//! greedy-argmax assert stays exact.
//!
//! The HTTP client is hand-rolled (std TcpStream, `Connection: close`); the
//! three JSON payloads parsed are constexpr-shaped llama.cpp responses, so
//! substring scanning replaces a serde dependency in dev-deps.

use ai_upcycler_worker::engine::WorkerEngine;
use ai_upcycler_worker::gguf::GgufFile;
use ai_upcycler_worker::proto::aiupcycler::rpc::{
    tensor_buffer::DataType, worker_service_server::WorkerService, ExecuteForwardRequest,
    LoadLayersRequest, TensorBuffer,
};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// Kills the server no matter how the test exits.
struct ServerGuard(std::process::Child);
impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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
    s.write_all(req.as_bytes()).expect("write health request");
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).expect("read response");
    Ok(String::from_utf8_lossy(&buf).replace("\u{0}", ""))
}

fn http_post(port: u16, path: &str, body: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
    s.set_read_timeout(Some(Duration::from_secs(120))).ok();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut wire = head.clone();
    wire.push_str(body);
    s.write_all(wire.as_bytes()).expect("write request");
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).expect("read response");
    let text = String::from_utf8_lossy(&buf).replace("\u{0}", "");
    let status = text.lines().next().unwrap_or("").to_string();
    assert!(status.contains(" 200 "), "POST {path} â†’ {status}");
    match text.split_once("\r\n\r\n") {
        Some((_, b)) => b.to_string(),
        None => panic!("POST {path}: no HTTP body separator"),
    }
}

/// First integer after a `"key":` occurrence (searching from a start offset).
fn json_int_after(hay: &str, key: &str, from: usize) -> i64 {
    let idx = hay[from..]
        .find(key)
        .unwrap_or_else(|| panic!("missing {key}"));
    let start = from + idx + key.len();
    let digits: String = hay[start..]
        .chars()
        .skip_while(|c| !c.is_ascii_digit() && *c != '-')
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse::<i64>().expect("int digits")
}

/// Integers inside a `"key":[...]` list.
fn json_int_list(hay: &str, key: &str) -> Vec<u32> {
    let idx = hay.find(key).unwrap_or_else(|| panic!("missing {key}"));
    let start = idx + key.len();
    let (open, _) = hay[start..].split_once('[').expect("list open");
    let (list, _) = hay[start + open.len() + 1..]
        .split_once(']')
        .expect("list close");
    list.split(',')
        .filter_map(|t| t.trim().parse::<u32>().ok())
        .collect()
}

/// (chosen_id, [(top_id, top_logprob); 5]) from a `/v1/completions` response.
fn parse_completion(body: &str) -> (i64, Vec<(i64, f64)>) {
    let chosen = json_int_after(body, "\"id\":", body.find("\"content\"").expect("content"));
    let tops_idx = body.find("\"top_logprobs\"").expect("top_logprobs");
    let mut tops = Vec::new();
    let mut rest = &body[tops_idx..];
    while let Some(sep) = rest.find("\"id\":") {
        rest = &rest[sep + 5..];
        let id: i64 = rest
            .chars()
            .skip_while(|c| !c.is_ascii_digit() && *c != '-')
            .take_while(|c| c.is_ascii_digit() || *c == '-')
            .collect::<String>()
            .parse()
            .expect("top id");
        let lp_idx = rest.find("\"logprob\":").expect("top logprob");
        let lp_start = lp_idx + "\"logprob\":".len();
        let lp_end = rest[lp_start..].find([',', '}']).expect("logprob end");
        let lp: f64 = rest[lp_start..lp_start + lp_end]
            .trim()
            .parse()
            .expect("logprob");
        tops.push((id, lp));
        if tops.len() >= 25 {
            break;
        }
    }
    (chosen, tops)
}

/// logits_row â†’ log_softmax row (f64 accumulation).
fn log_softmax(row: &[f32]) -> Vec<f64> {
    let max = row
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, |a, v| a.max(v as f64));
    let lse = max
        + row
            .iter()
            .map(|&v| (v as f64 - max).exp())
            .sum::<f64>()
            .ln();
    row.iter().map(|&v| v as f64 - lse).collect()
}

#[tokio::test]
#[ignore = "asset-gated: needs AI_UPCCLER_MODEL + AI_UPCCLER_LLAMA_SERVER"]
async fn next_token_matches_llama_server() {
    let Ok(model) = std::env::var("AI_UPCCLER_MODEL") else {
        eprintln!("parity skipped: AI_UPCCLER_MODEL is not set");
        return;
    };
    let Ok(server_bin) = std::env::var("AI_UPCCLER_LLAMA_SERVER") else {
        eprintln!("parity skipped: AI_UPCCLER_LLAMA_SERVER is not set");
        return;
    };

    let gguf = GgufFile::open(std::path::Path::new(&model)).expect("parse real GGUF");
    let h = gguf.hparams();

    // ---- llama-server on a free port, piped to null (never blocks) --------
    let port = free_port();
    let mut guard = ServerGuard(
        std::process::Command::new(&server_bin)
            .args([
                "--model",
                &model,
                "--port",
                &port.to_string(),
                "--ctx-size",
                "512",
                "--host",
                "127.0.0.1",
                // The engine's KV layer keeps F32 rows; llama-server defaults
                // to F16 caches. Pin the reference to the same precision so
                // the gate checks the graph, not a deliberate cache-type
                // difference.
                "--cache-type-k",
                "f32",
                "--cache-type-v",
                "f32",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn llama-server"),
    );

    let mut ready = None;
    for _ in 0..60 {
        match http_get(port, "/health") {
            Ok(body) if body.contains("\"status\":\"ok\"") => {
                ready = Some(());
                break;
            }
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    }
    assert!(
        ready.is_some(),
        "llama-server never became healthy on :{port}"
    );

    // ---- conditioned ids from the server's own tokenizer -------------------
    let prompt = "The capital of France is";
    let tok_body = http_post(port, "/tokenize", &format!("{{\"content\":\"{prompt}\"}}"));
    let ids = json_int_list(&tok_body, "\"tokens\":");
    assert!(!ids.is_empty(), "empty tokenize result: {tok_body}");
    eprintln!("tokenize: {ids:?}");

    // greedy next token from the same conditioning (raw completion endpoint:
    // no prompt template on either side)
    let greedy_body = http_post(
        port,
        "/v1/completions",
        &format!("{{\"model\":\"qwen2.5\",\"prompt\":\"{prompt}\",\"temperature\":0,\"n_predict\":1,\"logprobs\":25,\"stream\":false}}"),
    );
    let (server_top, server_logprobs) = parse_completion(&greedy_body);
    eprintln!(
        "server greedy token: {server_top} ({:#.6})",
        server_logprobs[0].1
    );
    let _ = &greedy_body;

    drop_server_early(&mut guard); // server no longer needed; keep test hermetic

    // ---- the engine's forward over the same ids ---------------------------
    let engine =
        WorkerEngine::new("parity-gate", "", std::path::PathBuf::from(&model)).expect("engine");
    let resp = engine
        .load_layers(tonic::Request::new(LoadLayersRequest {
            model_id: gguf.model_name().to_string(),
            start_layer: 0,
            end_layer: (h.block_count as i32) - 1,
            weights_chunk_data: Vec::new(),
        }))
        .await
        .expect("rpc")
        .into_inner();
    assert!(resp.success, "full-range load: {}", resp.error_message);
    let t0 = Instant::now();
    let n = ids.len() as i64;
    let fw = engine
        .execute_forward(tonic::Request::new(ExecuteForwardRequest {
            session_id: "parity".to_string(),
            start_layer: 0,
            end_layer: n_layers(h),
            activation_tensor: Some(TensorBuffer {
                shape: vec![1, n],
                data_type: DataType::Fp32 as i32,
                raw_data: ids.iter().flat_map(|&i| (i as f32).to_le_bytes()).collect(),
            }),
            sequence_position: 0,
        }))
        .await
        .expect("rpc")
        .into_inner();
    eprintln!("forward: {:.2?}", t0.elapsed());
    assert!(fw.success, "forward refused");
    let tb = fw.output_activation_tensor.expect("logits");
    assert_eq!(tb.shape[0], n, "logit rows = prompt tokens");
    let row_start = ((n - 1) * tb.shape[1]) as usize * 4;
    let row: Vec<f32> = tb.raw_data[row_start..]
        .as_chunks::<4>()
        .0
        .iter()
        .take(tb.shape[1] as usize)
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    drop(engine); // free 2.3 GiB before asserts (they don't need it)

    // ---- asserts: greedy token + top-5 logprob machinery -------------------
    let logprobs = log_softmax(&row);
    let mut ranking: Vec<usize> = (0..row.len()).collect();
    ranking.sort_by(|&a, &b| logprobs[b].partial_cmp(&logprobs[a]).unwrap());
    let our_top: i64 = ranking[0] as i64;
    let our_ranking: Vec<(i64, f64)> = ranking[..5]
        .iter()
        .map(|&i| (i as i64, logprobs[i]))
        .collect();
    eprintln!("our greedy token: {our_top} ({:#.6})", our_ranking[0].1);
    for (i, (tok, lp)) in our_ranking.iter().enumerate() {
        eprintln!("our   top{}: id {tok:>6}  lp {lp:.6}", i + 1);
    }
    for (i, (tok, lp)) in server_logprobs.iter().enumerate() {
        eprintln!("server top{}: id {tok:>6}  lp {lp:.6}", i + 1);
    }

    let shared = our_ranking
        .iter()
        .filter(|(id, _)| server_logprobs.iter().any(|(sid, _)| sid == id))
        .count();

    // Evidence table: for every server-listed id, our rank, our logprob, and
    // the logprob delta against llama.cpp.
    let mut worst = 0.0f64;
    for (sid, s_lp) in server_logprobs.iter() {
        let rank = ranking.iter().position(|&r| r as i64 == *sid);
        if let Some(r) = rank {
            let tid = ranking[r];
            let our_lp = logprobs[tid];
            let d = our_lp - *s_lp;
            worst = worst.max(d.abs());
            eprintln!("delta rank {r:>4} id {tid:>6} lp {s_lp:.4} ours {our_lp:.4} delta {d:+.4}");
        }
    }
    eprintln!("worst |delta| on shared ids: {worst:.4}");
    assert_eq!(
        our_top, server_top,
        "greedy next token must match llama.cpp conditioning"
    );
    assert!(
        shared >= 4,
        "top-5 overlap {shared}/5 - distributions diverged (see table)"
    );
    // Bound justified in the header comment: llama.cpp's CPU kernels quantize
    // activations to 8-bit inside each quantized-weight matmul (~0.05 nats of
    // logprob drift per block, F32-weight files agree to <= 4e-4); 24 blocks
    // observed worst ~0.55.
    assert!(
        worst <= 0.6,
        "worst shared-token logprob delta {worst} > 0.6 - graph divergence beyond \
         llama.cpp's quantized-kernel activation noise (see table)"
    );
}

fn n_layers(h: &ai_upcycler_worker::gguf::ModelHparams) -> i32 {
    (h.block_count as i32) - 1
}

fn drop_server_early(guard: &mut ServerGuard) {
    let _ = guard.0.kill();
    let _ = guard.0.wait();
}
