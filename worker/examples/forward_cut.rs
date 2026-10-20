//! Cuts the Android worker's committed forward-parity vectors (spec
//! 2026-10-19 android-compute-rpcs, D-A2): drives `WorkerEngine` on the tiny
//! committed fixtures and writes one text file per case holding each step's
//! input activation and the engine's output activation. The Kotlin parity
//! test replays them at the spec's tolerance; `worker/tests/forward_parity.rs`
//! replays them here as the Rust regression guard.
//!
//! Run from the repo root (paths below are repo-root relative) with:
//! `cargo run -p ai-upcycler-worker --example forward_cut`
//!
//! Outputs are never hand-edited.

use ai_upcycler_worker::engine::WorkerEngine;
use ai_upcycler_worker::proto::aiupcycler::rpc::tensor_buffer::DataType;
use ai_upcycler_worker::proto::aiupcycler::rpc::worker_service_server::WorkerService;
use ai_upcycler_worker::proto::aiupcycler::rpc::{
    ExecuteForwardRequest, LoadLayersRequest, TensorBuffer,
};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use tonic::Request;

/// Single source of truth for fixtures and vectors (D-A2); both live under
/// the Android unit-test resources.
const FIXTURE_DIR: &str = "mobile-worker/android/app/src/test/resources/engine/fixtures";
const OUT_DIR: &str = "mobile-worker/android/app/src/test/resources/engine/forward";

const HIDDEN: i64 = 64;

struct Step {
    /// `sequence_position` on the wire.
    pos: i32,
    shape: Vec<i64>,
    vals: Vec<f32>,
}

struct Case {
    name: &'static str,
    fixture: &'static str,
    load: (i32, i32),
    steps: Vec<Step>,
}

fn ids(v: &[u32]) -> Vec<f32> {
    v.iter().map(|&i| i as f32).collect()
}

/// Deterministic synthetic hidden activation: `(i + offset) % 7 - 3`, the
/// same shape of input `worker/tests/forward.rs` feeds a mid stage.
fn synth(len: usize, offset: usize) -> Vec<f32> {
    (0..len).map(|i| ((i + offset) % 7) as f32 - 3.0).collect()
}

/// Little-endian IEEE-754 bytes as lowercase hex (the format both replays
/// parse back with `f32::from_le_bytes` / `Float.intBitsToFloat`).
fn hex_f32(vals: &[f32]) -> String {
    let mut out = String::with_capacity(vals.len() * 8);
    for v in vals {
        for b in v.to_le_bytes() {
            write!(out, "{b:02x}").expect("string write");
        }
    }
    out
}

fn f32_tb(shape: &[i64], vals: &[f32]) -> TensorBuffer {
    assert_eq!(
        shape.iter().product::<i64>() as usize,
        vals.len(),
        "shape/value mismatch"
    );
    TensorBuffer {
        shape: shape.to_vec(),
        data_type: DataType::Fp32 as i32,
        raw_data: vals.iter().flat_map(|v| v.to_le_bytes()).collect(),
    }
}

fn decode_f32(raw: &[u8]) -> Vec<f32> {
    assert!(raw.len().is_multiple_of(4), "raw bytes not f32-aligned");
    raw.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn case_definitions() -> Vec<Case> {
    let untied = "tiny-untied-f32.gguf";
    let tied = "tiny-tied-q4-0.gguf";
    vec![
        // Stage-0 prefill then decode on a partial (non-head) range: hidden
        // out both times, KV cache grows between the steps.
        Case {
            name: "stage0_prefill_decode",
            fixture: untied,
            load: (0, 1),
            steps: vec![
                Step {
                    pos: 0,
                    shape: vec![1, 3],
                    vals: ids(&[1, 3, 2]),
                },
                Step {
                    pos: 3,
                    shape: vec![1, 1],
                    vals: ids(&[4]),
                },
            ],
        },
        // Mid stage: hidden in, hidden out, session continuity across steps.
        Case {
            name: "mid_stage",
            fixture: untied,
            load: (2, 2),
            steps: vec![
                Step {
                    pos: 0,
                    shape: vec![2, HIDDEN],
                    vals: synth(2 * HIDDEN as usize, 0),
                },
                Step {
                    pos: 2,
                    shape: vec![1, HIDDEN],
                    vals: synth(HIDDEN as usize, 17),
                },
            ],
        },
        // Head stage alone (untied): hidden in, logits [n, vocab] out.
        Case {
            name: "head_stage_logits",
            fixture: untied,
            load: (3, 3),
            steps: vec![
                Step {
                    pos: 0,
                    shape: vec![2, HIDDEN],
                    vals: synth(2 * HIDDEN as usize, 0),
                },
                Step {
                    pos: 2,
                    shape: vec![1, HIDDEN],
                    vals: synth(HIDDEN as usize, 17),
                },
            ],
        },
        // Full range on the tied Q4_0 fixture: exercises Q4_0 dequant of the
        // embeddings/projections feeding real matmuls, the QKV bias add path,
        // and the tied (token_embd-reused) head in one case.
        Case {
            name: "full_range_tied_q4",
            fixture: tied,
            load: (0, 1),
            steps: vec![Step {
                pos: 0,
                shape: vec![1, 3],
                vals: ids(&[1, 3, 2]),
            }],
        },
    ]
}

fn render_case(case: &Case, outputs: &[(Vec<i64>, Vec<f32>)]) -> String {
    let mut s = String::new();
    s.push_str("# android forward-parity vector, cut from the Rust engine by\n");
    s.push_str("# worker/examples/forward_cut.rs (spec 2026-10-19 android-compute-rpcs,\n");
    s.push_str("# D-A2). NEVER hand-edited: regenerate from the repo root with\n");
    s.push_str("# `cargo run -p ai-upcycler-worker --example forward_cut`. Sections in\n");
    s.push_str("# brackets, key|value lines; *_hex is little-endian IEEE-754 bytes of the\n");
    s.push_str("# F32 payload, *_shape is row-major with the stage's wire semantics.\n");
    s.push_str("[case]\n");
    writeln!(s, "fixture|{}", case.fixture).expect("string write");
    writeln!(s, "load_range|{},{}", case.load.0, case.load.1).expect("string write");
    writeln!(s, "session|{}", case.name).expect("string write");
    for (i, step) in case.steps.iter().enumerate() {
        let (out_shape, out_vals) = &outputs[i];
        s.push_str(&format!("[step {i}]\n"));
        writeln!(s, "sequence_position|{}", step.pos).expect("string write");
        writeln!(
            s,
            "input_shape|{}",
            step.shape
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
        .expect("string write");
        writeln!(s, "input_hex|{}", hex_f32(&step.vals)).expect("string write");
        writeln!(
            s,
            "output_shape|{}",
            out_shape
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
        .expect("string write");
        writeln!(s, "output_hex|{}", hex_f32(out_vals)).expect("string write");
    }
    s
}

fn main() {
    let root = std::env::current_dir().expect("cwd");
    assert!(
        root.join(FIXTURE_DIR).is_dir(),
        "run from the repo root: {FIXTURE_DIR} not found under {}",
        root.display()
    );
    let out_dir = root.join(OUT_DIR);
    std::fs::create_dir_all(&out_dir).expect("create forward vector dir");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async move {
        for case in case_definitions() {
            let fixture_path: PathBuf = root.join(FIXTURE_DIR).join(case.fixture);
            let model_id = ai_upcycler_worker::gguf::GgufFile::open(Path::new(&fixture_path))
                .expect("fixture parses")
                .model_name()
                .to_string();
            let engine = WorkerEngine::new(case.name, "", fixture_path).expect("engine constructs");

            let load = engine
                .load_layers(Request::new(LoadLayersRequest {
                    model_id,
                    start_layer: case.load.0,
                    end_layer: case.load.1,
                    weights_chunk_data: Vec::new(),
                }))
                .await
                .expect("load rpc")
                .into_inner();
            assert!(
                load.success,
                "load {}..={} refused for {}: {}",
                case.load.0, case.load.1, case.name, load.error_message
            );

            let mut outputs = Vec::with_capacity(case.steps.len());
            for (i, step) in case.steps.iter().enumerate() {
                let resp = engine
                    .execute_forward(Request::new(ExecuteForwardRequest {
                        session_id: case.name.to_string(),
                        start_layer: case.load.0,
                        end_layer: case.load.1,
                        activation_tensor: Some(f32_tb(&step.shape, &step.vals)),
                        sequence_position: step.pos,
                    }))
                    .await
                    .expect("forward rpc")
                    .into_inner();
                assert!(
                    resp.success,
                    "forward {}/step {i} refused: output tensor missing on a refusal",
                    case.name
                );
                let tb = resp
                    .output_activation_tensor
                    .expect("successful forward must carry an output tensor");
                assert_eq!(tb.data_type, DataType::Fp32 as i32, "output dtype");
                let vals = decode_f32(&tb.raw_data);
                assert_eq!(
                    tb.shape.iter().product::<i64>() as usize,
                    vals.len(),
                    "output shape/value mismatch"
                );
                outputs.push((tb.shape, vals));
            }

            let path = out_dir.join(format!("{}.txt", case.name));
            std::fs::write(&path, render_case(&case, &outputs)).expect("write vector file");
            println!("cut {}", path.display());
        }
    });
}
