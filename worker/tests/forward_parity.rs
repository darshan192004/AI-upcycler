//! Committed forward-vector regression guard (spec 2026-10-19
//! android-compute-rpcs, Testing Strategy "Structural (Rust)"): replays
//! `mobile-worker/android/app/src/test/resources/engine/forward/*.txt`
//! against the Rust engine, so a Rust edit that changes forward outputs
//! fails HERE first - before the Kotlin parity copy could hide the drift.
//!
//! Fixture bytes are read from the same committed single source D-A2 names
//! (no regeneration step can drift from what the Kotlin test loads).
//!
//! Tolerance: the vectors were cut by `worker/examples/forward_cut.rs` on
//! one host; this guard replays them on whatever host runs CI. Dequant and
//! ordered multiply-accumulates are bitwise deterministic, but the forward
//! pass crosses a libm boundary (`powf`/`sinf`/`cosf`/`expf` in
//! RoPE/softmax/SiLU differ 1-2 ulp across MSVC/glibc/fdlibm), so outputs
//! are compared at the spec D-A2 amendment's bar: relative <= 1e-5 (~2 ulp)
//! with a small absolute floor near zero - never a blanket epsilon. On the
//! cutting host the replay is bit-exact; `--nocapture` prints the worst
//! relative deviation actually observed.

use ai_upcycler_worker::engine::WorkerEngine;
use ai_upcycler_worker::proto::aiupcycler::rpc::tensor_buffer::DataType;
use ai_upcycler_worker::proto::aiupcycler::rpc::worker_service_server::WorkerService;
use ai_upcycler_worker::proto::aiupcycler::rpc::{
    ExecuteForwardRequest, LoadLayersRequest, TensorBuffer,
};
use std::path::{Path, PathBuf};
use tonic::Request;

/// The committed single source (D-A2): fixtures and vectors both live in the
/// Android unit-test resources. Tests run with cwd = the worker crate root.
fn engine_resources() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mobile-worker/android/app/src/test/resources/engine")
}

/// Spec coverage that must exist (stage-0 prefill+decode, mid, head, and a
/// full range over the quantized tied fixture). Extra vector files are
/// replayed too; deleting one fails `all_spec_vectors_present`.
const REQUIRED_CASES: &[&str] = &[
    "stage0_prefill_decode",
    "mid_stage",
    "head_stage_logits",
    "full_range_tied_q4",
];

struct StepFile {
    pos: i32,
    in_shape: Vec<i64>,
    input: Vec<f32>,
    out_shape: Vec<i64>,
    output: Vec<f32>,
}

struct CaseFile {
    fixture: String,
    load: (i32, i32),
    session: String,
    steps: Vec<StepFile>,
}

fn decode_hex_f32(hex: &str, what: &str) -> Vec<f32> {
    assert!(
        hex.len().is_multiple_of(8),
        "{what}: hex length {} not a multiple of 8",
        hex.len()
    );
    let bytes: Vec<u8> = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| {
            let hi = (p[0] as char)
                .to_digit(16)
                .unwrap_or_else(|| panic!("{what}: bad hex digit"));
            let lo = (p[1] as char)
                .to_digit(16)
                .unwrap_or_else(|| panic!("{what}: bad hex digit"));
            ((hi << 4) | lo) as u8
        })
        .collect();
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn parse_shape(v: &str, what: &str) -> Vec<i64> {
    v.split(',')
        .map(|d| {
            d.parse::<i64>()
                .unwrap_or_else(|_| panic!("{what}: bad dimension {d}"))
        })
        .collect()
}

fn parse_case(text: &str) -> CaseFile {
    let mut fixture = String::new();
    let mut session = String::new();
    let mut load = (0i32, 0i32);
    let mut case_section = true;
    let mut steps: Vec<StepFile> = Vec::new();
    let mut cur: Vec<(String, String)> = Vec::new();

    let flush = |cur: &mut Vec<(String, String)>, steps: &mut Vec<StepFile>| {
        if cur.is_empty() {
            return;
        }
        let get = |key: &str| {
            cur.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("step missing key {key}"))
        };
        steps.push(StepFile {
            pos: get("sequence_position").parse().expect("sequence_position"),
            in_shape: parse_shape(&get("input_shape"), "input_shape"),
            input: decode_hex_f32(&get("input_hex"), "input_hex"),
            out_shape: parse_shape(&get("output_shape"), "output_shape"),
            output: decode_hex_f32(&get("output_hex"), "output_hex"),
        });
        cur.clear();
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[case]" {
            case_section = true;
            continue;
        }
        if let Some(n) = line.strip_prefix("[step ") {
            n.strip_suffix(']').expect("step header closes with ]");
            flush(&mut cur, &mut steps);
            case_section = false;
            continue;
        }
        let (k, v) = line.split_once('|').expect("key|value line");
        if case_section {
            match k {
                "fixture" => fixture = v.to_string(),
                "session" => session = v.to_string(),
                "load_range" => {
                    let (a, b) = v.split_once(',').expect("load_range start,end");
                    load = (a.parse().expect("start"), b.parse().expect("end"));
                }
                other => panic!("unknown case key {other}"),
            }
        } else {
            cur.push((k.to_string(), v.to_string()));
        }
    }
    flush(&mut cur, &mut steps);

    assert!(!fixture.is_empty(), "case fixture missing");
    assert!(!session.is_empty(), "case session missing");
    assert!(!steps.is_empty(), "case has no steps");
    CaseFile {
        fixture,
        load,
        session,
        steps,
    }
}

/// The D-A2 amendment's comparison bar: relative <= 1e-5 with an absolute
/// floor for the near-zero region where a relative comparison is
/// meaningless. Both sides (this test and the Kotlin parity test) use the
/// identical formula.
fn close_enough(a: f32, b: f32) -> bool {
    let tol = 1e-7f32.max(1e-5f32 * a.abs().max(b.abs()));
    (a - b).abs() <= tol
}

fn vector_files() -> Vec<PathBuf> {
    let dir = engine_resources().join("forward");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("vector dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    files.sort();
    files
}

#[test]
fn all_spec_vectors_present() {
    let files = vector_files();
    assert!(!files.is_empty(), "no committed forward vectors found");
    for name in REQUIRED_CASES {
        assert!(
            files
                .iter()
                .any(|p| p.file_stem().is_some_and(|s| s == *name)),
            "missing required vector case {name}.txt - regenerate with \
             `cargo run -p ai-upcycler-worker --example forward_cut`"
        );
    }
}

#[tokio::test]
async fn committed_vectors_replay_within_tolerance() {
    let files = vector_files();
    assert!(!files.is_empty(), "no committed forward vectors found");

    for path in files {
        let case_name = path
            .file_stem()
            .expect("stem")
            .to_string_lossy()
            .to_string();
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let case = parse_case(&text);

        let fixture_path = engine_resources().join("fixtures").join(&case.fixture);
        let model_id = ai_upcycler_worker::gguf::GgufFile::open(&fixture_path)
            .unwrap_or_else(|e| panic!("fixture {} parses: {e}", case.fixture))
            .model_name()
            .to_string();
        let engine = WorkerEngine::new(&case_name, "", fixture_path).expect("engine constructs");

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
            "{case_name}: load {}..={} refused: {}",
            case.load.0, case.load.1, load.error_message
        );

        let mut worst_abs = 0f32;
        let mut worst_bar_use = 0f32;
        let mut worst_rel = 0f32;
        for (i, step) in case.steps.iter().enumerate() {
            let resp = engine
                .execute_forward(Request::new(ExecuteForwardRequest {
                    session_id: case.session.clone(),
                    start_layer: case.load.0,
                    end_layer: case.load.1,
                    activation_tensor: Some(TensorBuffer {
                        shape: step.in_shape.clone(),
                        data_type: DataType::Fp32 as i32,
                        raw_data: step.input.iter().flat_map(|v| v.to_le_bytes()).collect(),
                    }),
                    sequence_position: step.pos,
                    last_logits_only: false,
                }))
                .await
                .expect("forward rpc")
                .into_inner();
            assert!(
                resp.success,
                "{case_name} step {i}: replay refused (the committed inputs must stay valid)"
            );
            let tb = resp.output_activation_tensor.expect("output tensor");
            assert_eq!(
                tb.shape, step.out_shape,
                "{case_name} step {i}: output shape drifted"
            );
            let got: Vec<f32> = tb
                .raw_data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect();
            assert_eq!(
                got.len(),
                step.output.len(),
                "{case_name} step {i}: output length drifted"
            );
            for (j, (want, got)) in step.output.iter().zip(&got).enumerate() {
                assert!(
                    want.is_finite() && got.is_finite(),
                    "{case_name} step {i} out[{j}]: non-finite want {want} got {got}"
                );
                assert!(
                    close_enough(*want, *got),
                    "{case_name} step {i} out[{j}]: committed {want} vs replayed {got} \
                     (delta {} exceeds the 1e-5 relative bar)",
                    (want - got).abs()
                );
                let diff = (want - got).abs();
                let tol = 1e-7f32.max(1e-5f32 * want.abs().max(got.abs()));
                worst_abs = worst_abs.max(diff);
                worst_bar_use = worst_bar_use.max(diff / tol);
                if want.abs().max(got.abs()) >= 1e-2 {
                    worst_rel = worst_rel.max(diff / want.abs().max(got.abs()));
                }
            }
        }
        eprintln!(
            "{case_name}: replay ok - worst rel (|v|>=1e-2) {worst_rel:.2e}, \
             worst abs delta {worst_abs:.2e}, max bar use {:.0}%",
            worst_bar_use * 100.0
        );
    }
}
