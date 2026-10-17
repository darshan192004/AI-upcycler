//! Split equivalence over real worker processes (plan Task 7).
//!
//! The epic's core guarantee, proven against the REAL binary + gRPC path
//! (not the in-process server harness other suites use): one model's block
//! range spread across several workers, activations threaded between them
//! by the master-side `LoadedPipeline` driver, computes the same logits as
//! a single worker holding every block. Identical F32 ops in identical
//! order must match to near bit-exactness; a real gap means a stage-
//! boundary conversion crept in.

mod common;

use ai_upcycler_core::rpc::engine::{ActivationType, LoadedPipeline, TensorPayload};
use ai_upcycler_core::scheduler::planner::LayerAssignment;
use common::{ids_payload, spawn_worker, write_tiny_model, TinyModelConfig};

fn assignment(node_id: &str, port: u16, start: usize, end: usize) -> LayerAssignment {
    LayerAssignment {
        node_id: node_id.to_string(),
        device_name: format!("{node_id} device"),
        ip_address: "127.0.0.1".to_string(),
        port,
        start_layer: start,
        end_layer: end,
        total_ram_used_bytes: 1 << 28,
        estimated_latency_ms: 1.0,
    }
}

/// Loads `plan`, pushes one stage-0 forward, unloads, and returns the final
/// tensor exactly as the driver produced it.
async fn run_split(
    model_id: &str,
    plan: &[LayerAssignment],
    session: &str,
    ids: &[u32],
) -> TensorPayload {
    let mut pipeline = LoadedPipeline::load(model_id, plan)
        .await
        .expect("load pipeline");
    let result = pipeline
        .forward(ids_payload(session, ids))
        .await
        .expect("forward through the whole plan");
    pipeline.unload().await.expect("unload pipeline");
    TensorPayload {
        session_id: result.session_id,
        sequence_index: result.sequence_index,
        shape: result.output_shape,
        data_bytes: result.output_bytes,
        data_type: result.output_data_type,
    }
}

fn f32_values(payload: &TensorPayload) -> Vec<f32> {
    assert_eq!(payload.data_type, ActivationType::Fp32, "logits dtype");
    payload
        .data_bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn kill(mut child: std::process::Child) {
    child.kill().expect("kill worker child");
    child.wait().expect("reap worker child");
}

#[tokio::test]
async fn two_worker_split_matches_single_worker() {
    let dir = tempfile::tempdir().expect("tempdir");
    // default config: 4 blocks, tied head, F32 fixture
    let fx = write_tiny_model(dir.path(), TinyModelConfig::default());
    let ids = [3u32, 1, 2, 1];

    // single: one process owns layers 0..3
    let (port_a, child_a) = spawn_worker(&fx.path, "split-a");
    let out_single = run_split(
        &fx.model_id,
        &[assignment("split-a", port_a, 0, 3)],
        "single-1",
        &ids,
    )
    .await;
    kill(child_a);

    // dual: two processes, 0..1 and 2..3
    let (port_b, child_b) = spawn_worker(&fx.path, "split-b");
    let (port_c, child_c) = spawn_worker(&fx.path, "split-c");
    let out_dual = run_split(
        &fx.model_id,
        &[
            assignment("split-b", port_b, 0, 1),
            assignment("split-c", port_c, 2, 3),
        ],
        "split-1",
        &ids,
    )
    .await;
    kill(child_b);
    kill(child_c);

    assert_eq!(
        out_single.shape,
        vec![ids.len() as i64, 256],
        "[rows, vocab] logits shape"
    );
    assert_eq!(
        out_dual.shape, out_single.shape,
        "split must keep the shape"
    );
    let a = f32_values(&out_single);
    let b = f32_values(&out_dual);
    assert_eq!(a.len(), 4 * 256);
    let max_diff = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(max_diff <= 1e-6, "split changed the math: {max_diff}");
}

/// Real-model sibling: the 24-block Qwen2.5-0.5B file split into two worker
/// processes at block 12 must reproduce the single-process logits (and the
/// same greedy argmax). Needs the same assets as `real_model.rs` /
/// `parity.rs`; runs only with `AI_UPCCLER_MODEL` set.
#[tokio::test]
#[ignore = "needs the real-model asset (AI_UPCCLER_MODEL)"]
async fn real_model_two_worker_split_matches_single_worker() {
    let Ok(model) = std::env::var("AI_UPCCLER_MODEL") else {
        eprintln!("split-equivalence skipped: AI_UPCCLER_MODEL is not set");
        return;
    };
    let model_path = std::path::PathBuf::from(&model);
    let gguf = ai_upcycler_worker::gguf::GgufFile::open(&model_path).expect("parse real GGUF");
    let model_id = gguf.model_name().to_string();
    let blocks = gguf.hparams().block_count as usize;
    let split_at = blocks / 2; // 12 for the 24-block Qwen2.5-0.5B
                               // "The capital of France is" — the same ids the parity gate uses.
    let ids = [785u32, 6722, 315, 9625, 374];

    // single: one process owns every block
    let (port_a, child_a) = spawn_worker(&model_path, "split-real-a");
    let out_single = run_split(
        &model_id,
        &[assignment("split-real-a", port_a, 0, blocks - 1)],
        "single-1",
        &ids,
    )
    .await;
    kill(child_a); // free ~2.3 GiB before the dual run

    // dual: two processes, 0..split_at-1 and split_at..blocks-1
    let (port_b, child_b) = spawn_worker(&model_path, "split-real-b");
    let (port_c, child_c) = spawn_worker(&model_path, "split-real-c");
    let out_dual = run_split(
        &model_id,
        &[
            assignment("split-real-b", port_b, 0, split_at - 1),
            assignment("split-real-c", port_c, split_at, blocks - 1),
        ],
        "split-1",
        &ids,
    )
    .await;
    kill(child_b);
    kill(child_c);

    assert_eq!(
        out_single.shape, out_dual.shape,
        "split must keep the shape"
    );
    let a = f32_values(&out_single);
    let b = f32_values(&out_dual);
    assert_eq!(a.len(), b.len());
    let max_diff = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    eprintln!("real-model split max_diff: {max_diff}");
    assert!(max_diff <= 1e-4, "split changed the math: {max_diff}");
    assert_eq!(
        argmax(&a),
        argmax(&b),
        "greedy argmax must be split-independent"
    );
}

fn argmax(v: &[f32]) -> usize {
    (0..v.len())
        .max_by(|&i, &j| v[i].total_cmp(&v[j]))
        .expect("non-empty logits")
}
