//! Writes the committed engine fixtures consumed by the Android worker's
//! tests (see `fixtures/engine/` and the 2026-10-19 Android compute-RPCs
//! spec, D-A2). Regenerates byte-identically (xorshift32 weights from a
//! fixed seed, fixed KV tables); outputs are never hand-edited. Run from the
//! repo root with: `cargo run -p ai-upcycler-model-io --example fixture_dump
//! -- fixtures/engine`.

use ai_upcycler_model_io::fixture::{
    write_tiny_model, FixtureQuant, TinyModelConfig, TokenizerFixture,
};

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: fixture_dump <out-dir>");
    let dir = std::path::Path::new(&dir);
    std::fs::create_dir_all(dir).expect("create output dir");

    // 1. Untied F32 head + tokenizer KVs covering the reader's full KV skip
    //    matrix (string arrays, f32 arrays, i32 token-type arrays).
    let f1 = write_tiny_model(
        dir,
        TinyModelConfig {
            n_layers: 4,
            tied: false,
            quant: FixtureQuant::F32,
            model_name: "tiny-untied-f32".to_string(),
            tokenizer: Some(TokenizerFixture {
                include_scores: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    );

    // 2. Tied Q4_0 with attention QKV biases (Qwen 0.5B-class extras), so the
    //    engine's bias-add path and Q4_0 dequant share one file.
    let f2 = write_tiny_model(
        dir,
        TinyModelConfig {
            n_layers: 2,
            tied: true,
            quant: FixtureQuant::Q4_0,
            model_name: "tiny-tied-q4-0".to_string(),
            qkv_bias: true,
            ..Default::default()
        },
    );

    // 3. Load-refusal fixture: the projections are Q5_1 (ggml type id 7),
    //    outside the engine's supported set.
    let f3 = write_tiny_model(
        dir,
        TinyModelConfig {
            n_layers: 1,
            quant: FixtureQuant::UnsupportedQ5_1,
            model_name: "tiny-q5-1-refusal".to_string(),
            tokenizer: None,
            ..Default::default()
        },
    );

    for f in [&f1, &f2, &f3] {
        println!(
            "{:?} model_id={} n_layers={} tied={}",
            f.path, f.model_id, f.n_layers, f.tied
        );
    }
}
