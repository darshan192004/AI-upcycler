//! Asset-gated real-model gate (plan Task 4 Step 5): parses a real Qwen2.5
//! GGUF, checks it against the supported type set, benchmarks a full-range
//! load (< 25 s budget), and verifies every dequantized value is finite.
//!
//! Skips with a printed message unless `AI_UPCYCLER_MODEL` points at a
//! real `.gguf` file (CI never sets it; local runs opt in).

use ai_upcycler_worker::engine::WorkerEngine;
use ai_upcycler_worker::gguf::GgufFile;
use ai_upcycler_worker::proto::aiupcycler::rpc::{
    worker_service_server::WorkerService, LoadLayersRequest,
};
use ai_upcycler_worker::quant::{dequantize, GgmlType};
use std::time::{Duration, Instant};

#[tokio::test]
async fn real_model_header_and_full_load() {
    let Ok(path) = std::env::var("AI_UPCYCLER_MODEL") else {
        eprintln!("real-model gate skipped: AI_UPCYCLER_MODEL is not set");
        return;
    };

    let gguf = GgufFile::open(std::path::Path::new(&path)).expect("parse real GGUF header");

    // Qwen2.5-0.5B constants, cross-checked against the independent oracle
    // (Qwen/Qwen2.5-0.5B config.json on Hugging Face, fetched 2026-10-17):
    // num_hidden_layers 24, hidden_size 896, num_attention_heads 14,
    // num_key_value_heads 2, intermediate_size 4864,
    // max_position_embeddings 32768, tie_word_embeddings true.
    let h = gguf.hparams();
    assert_eq!(h.block_count, 24, "block_count");
    assert_eq!(h.embedding_length, 896, "embedding_length");
    assert_eq!(h.head_count, 14, "head_count");
    assert_eq!(h.head_count_kv, 2, "head_count_kv");
    assert_eq!(h.feed_forward_length, 4864, "feed_forward_length");
    assert!(
        h.context_length >= 8192,
        "context_length {} >= 8192",
        h.context_length
    );
    eprintln!(
        "real model header ok: {} blocks x {} hidden, ctx {}, rope theta {}",
        h.block_count, h.embedding_length, h.context_length, h.rope_theta
    );

    // Every tensor must be in the supported set before any load happens.
    let mut tensor_count = 0usize;
    let mut type_histogram: Vec<(u32, usize)> = Vec::new();
    for name in gguf.tensor_names() {
        let desc = gguf.tensor(name).expect("name from the same table");
        assert!(
            GgmlType::from_id(desc.ggml_type).is_some(),
            "tensor {name} has unsupported ggml type id {}",
            desc.ggml_type
        );
        tensor_count += 1;
        match type_histogram
            .iter_mut()
            .find(|(id, _)| *id == desc.ggml_type)
        {
            Some(entry) => entry.1 += 1,
            None => type_histogram.push((desc.ggml_type, 1)),
        }
    }
    let histogram = type_histogram
        .iter()
        .map(|(id, n)| format!("id {id} x{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    eprintln!("all {tensor_count} tensor types supported: {histogram}");

    // Load-budget benchmark: full-range materialization < 25 s.
    let engine =
        WorkerEngine::new("real-gate", "", std::path::PathBuf::from(&path)).expect("engine");
    let t0 = Instant::now();
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
    let elapsed = t0.elapsed();
    assert!(resp.success, "full load: {}", resp.error_message);
    let gib = resp.allocated_ram_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    eprintln!("full-range load: {elapsed:.2?}, {gib:.3} GiB allocated (budget 25s)");
    if cfg!(debug_assertions) {
        eprintln!("debug-profile build: budget skipped (measure with `cargo test --release`)");
    } else {
        assert!(
            elapsed < Duration::from_secs(25),
            "load took {elapsed:?}, budget 25s"
        );
    }
    drop(engine); // free retained weights before the finiteness sweep

    // Every dequantized value from the real file must be finite.
    for name in gguf.tensor_names() {
        let desc = gguf.tensor(name).expect("name from the same table");
        let ty = GgmlType::from_id(desc.ggml_type).expect("checked above");
        let raw = gguf.read_raw(name).expect("read tensor bytes");
        let values =
            dequantize(ty, &raw, &desc.ne).unwrap_or_else(|e| panic!("dequant {name}: {e}"));
        assert_eq!(
            values.len() as u64,
            desc.ne.iter().product::<u64>(),
            "length of {name}"
        );
        assert!(
            values.iter().all(|v| v.is_finite()),
            "non-finite values in {name}"
        );
    }
    eprintln!("finiteness sweep over all {tensor_count} tensors ok");
}
