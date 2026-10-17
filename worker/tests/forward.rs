//! ExecuteForward contract tests (plan Task 6): stage roles, session KV
//! caches, position gates, and one f64-reference block-math check against
//! the real in-process gRPC engine.

mod common;

use ai_upcycler_worker::engine::WorkerEngine;
use ai_upcycler_worker::proto::aiupcycler::rpc::tensor_buffer::DataType;
use ai_upcycler_worker::proto::aiupcycler::rpc::{
    worker_service_client::WorkerServiceClient, ExecuteForwardRequest, ExecuteForwardResponse,
    LoadLayersRequest, TensorBuffer,
};

type Client = WorkerServiceClient<tonic::transport::Channel>;

async fn connect(fx: &common::Fixture) -> Client {
    let engine = WorkerEngine::new("forward-1", "test-device", fx.path.clone()).expect("engine");
    let (port, _server) = common::serve_engine(engine).await;
    WorkerServiceClient::connect(format!("http://127.0.0.1:{port}"))
        .await
        .expect("connect")
}

async fn load_ok(client: &mut Client, fx: &common::Fixture, start: i32, end: i32) {
    let resp = client
        .load_layers(LoadLayersRequest {
            model_id: fx.model_id.clone(),
            start_layer: start,
            end_layer: end,
            weights_chunk_data: Vec::new(),
        })
        .await
        .expect("rpc")
        .into_inner();
    assert!(
        resp.success,
        "load {start}..={end} must succeed: {}",
        resp.error_message
    );
}

async fn forward(
    client: &mut Client,
    sid: &str,
    start: i32,
    end: i32,
    tb: TensorBuffer,
    seq_pos: i32,
) -> ExecuteForwardResponse {
    client
        .execute_forward(ExecuteForwardRequest {
            session_id: sid.to_string(),
            start_layer: start,
            end_layer: end,
            activation_tensor: Some(tb),
            sequence_position: seq_pos,
        })
        .await
        .expect("rpc")
        .into_inner()
}

/// Row-major F32 input buffer, little-endian (the engine's byte order).
fn f32_buf(shape: &[i64], vals: &[f32]) -> TensorBuffer {
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

/// Decodes a successful response back into F32 (little-endian).
fn out_f32(tb: &TensorBuffer) -> Vec<f32> {
    assert_eq!(tb.data_type, DataType::Fp32 as i32, "output dtype");
    tb.raw_data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn ids_buf(ids: &[u32]) -> TensorBuffer {
    f32_buf(
        &[1, ids.len() as i64],
        &ids.iter().map(|&i| i as f32).collect::<Vec<_>>(),
    )
}

// ---- stage roles -----------------------------------------------------------

#[tokio::test]
async fn stage0_full_range_returns_logits() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(
        dir.path(),
        common::TinyModelConfig {
            n_layers: 1,
            ..Default::default()
        },
    );
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 0, 0).await;

    let resp = forward(&mut client, "s1", 0, 0, ids_buf(&[1, 2]), 0).await;
    assert!(resp.success, "forward: {}", resp.execution_time_us);
    let out = resp.output_activation_tensor.expect("output");
    assert_eq!(out.shape, vec![2, common::VOCAB as i64]);
    assert_eq!(out.data_type, DataType::Fp32 as i32);
    assert_eq!(out.raw_data.len(), 2 * common::VOCAB * 4);
    assert!(resp.execution_time_us > 0, "real pass must report time");
}

#[tokio::test]
async fn partial_stage0_returns_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 0, 0).await;

    let resp = forward(&mut client, "s1", 0, 0, ids_buf(&[1, 3, 2]), 0).await;
    assert!(resp.success);
    let out = resp.output_activation_tensor.expect("output");
    assert_eq!(out.shape, vec![3, common::HIDDEN as i64]);
}

#[tokio::test]
async fn mid_stage_consumes_hidden_and_rejects_roles() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 2, 2).await;

    // [3, hidden] hidden input → [3, hidden] output.
    let hidden: Vec<f32> = (0..3 * common::HIDDEN)
        .map(|i| (i % 7) as f32 - 3.0)
        .collect();
    let resp = forward(
        &mut client,
        "m1",
        2,
        2,
        f32_buf(&[3, common::HIDDEN as i64], &hidden),
        0,
    )
    .await;
    assert!(resp.success);
    let out = resp.output_activation_tensor.expect("output");
    assert_eq!(out.shape, vec![3, common::HIDDEN as i64]);

    // Wrong role for a mid stage: token ids — must be refused.
    let ids = forward(&mut client, "m1", 2, 2, ids_buf(&[1, 3]), 0).await;
    assert!(!ids.success, "mid stage must refuse ids input");

    // Wrong dtype: FP16 hidden — must be refused (FP32 only).
    let mut fp16 = f32_buf(&[3, common::HIDDEN as i64], &hidden);
    fp16.data_type = DataType::Fp16 as i32;
    let fp16_resp = forward(&mut client, "m1", 2, 2, fp16, 0).await;
    assert!(!fp16_resp.success, "fp16 must be refused");
}

// ---- position gates ---------------------------------------------------------

#[tokio::test]
async fn forward_rejects_out_of_order_position_then_accepts_corrected() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 0, 3).await;

    let bad = forward(&mut client, "fresh", 0, 3, ids_buf(&[2]), 1).await;
    assert!(!bad.success, "kv_len 0 but position 1 requested");

    let ok0 = forward(&mut client, "fresh", 0, 3, ids_buf(&[2]), 0).await;
    assert!(ok0.success, "position 0 on a fresh session");

    let repeat = forward(&mut client, "fresh", 0, 3, ids_buf(&[2]), 0).await;
    assert!(
        !repeat.success,
        "kv cache at 1; position 0 already consumed"
    );

    let ok1 = forward(&mut client, "fresh", 0, 3, ids_buf(&[4]), 1).await;
    assert!(ok1.success, "position 1 == kv_len continues the sequence");
}

#[tokio::test]
async fn forward_rejects_token_id_at_or_above_vocab() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(
        dir.path(),
        common::TinyModelConfig {
            n_layers: 1,
            ..Default::default()
        },
    );
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 0, 0).await;

    let resp = forward(
        &mut client,
        "v1",
        0,
        0,
        ids_buf(&[1, common::VOCAB as u32]),
        0,
    )
    .await;
    assert!(
        !resp.success,
        "token id {0} == vocab must be refused",
        common::VOCAB
    );
}

#[tokio::test]
async fn forward_rejects_beyond_context_length() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 0, 3).await;

    // ctx = 64: a fresh session at position 64 already exceeds it (64 + 1 > ctx).
    let resp = forward(&mut client, "ctx", 0, 3, ids_buf(&[2]), 64).await;
    assert!(!resp.success);
}

// ---- sessions ---------------------------------------------------------------

#[tokio::test]
async fn session_cap_refuses_the_seventeenth_concurrent_session() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 0, 3).await;

    for i in 0..16 {
        let ok = forward(&mut client, &format!("cap-{i}"), 0, 3, ids_buf(&[1]), 0).await;
        assert!(ok.success, "session {i} must fit under the cap");
    }
    let over = forward(&mut client, "cap-16", 0, 3, ids_buf(&[1]), 0).await;
    assert!(!over.success, "17th concurrent session must be refused");
}

#[tokio::test]
async fn sessions_are_isolated_interleaved() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 0, 3).await;

    let a0 = forward(&mut client, "A", 0, 3, ids_buf(&[2]), 0).await;
    assert!(a0.success);
    let b0 = forward(&mut client, "B", 0, 3, ids_buf(&[2]), 0).await;
    assert!(b0.success);
    // Same tokens/positions in a different session → identical bytes.
    assert_eq!(
        a0.output_activation_tensor.expect("out A").raw_data,
        b0.output_activation_tensor.expect("out B").raw_data
    );

    let a1 = forward(&mut client, "A", 0, 3, ids_buf(&[4]), 1).await;
    assert!(a1.success);
    let b1 = forward(&mut client, "B", 0, 3, ids_buf(&[4]), 1).await;
    assert!(b1.success);
    assert_eq!(
        a1.output_activation_tensor.expect("out A").raw_data,
        b1.output_activation_tensor.expect("out B").raw_data
    );
}

#[tokio::test]
async fn unload_clears_sessions_and_reload_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 0, 3).await;
    let first = forward(&mut client, "u1", 0, 3, ids_buf(&[2]), 0).await;
    assert!(first.success);

    client
        .unload_layers(
            ai_upcycler_worker::proto::aiupcycler::rpc::UnloadLayersRequest {
                model_id: fx.model_id.clone(),
            },
        )
        .await
        .expect("rpc")
        .into_inner();

    let after = forward(&mut client, "u1", 0, 3, ids_buf(&[2]), 0).await;
    assert!(!after.success, "forward after unload must refuse");

    load_ok(&mut client, &fx, 0, 3).await;
    let reloaded = forward(&mut client, "u1", 0, 3, ids_buf(&[2]), 0).await;
    assert!(reloaded.success, "reload must start fresh sessions");
}

// ---- tie + head selection ----------------------------------------------------

#[tokio::test]
async fn tied_full_range_forward_returns_logits() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(
        dir.path(),
        common::TinyModelConfig {
            n_layers: 2,
            tied: true,
            ..Default::default()
        },
    );
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 0, 1).await;
    let resp = forward(&mut client, "t1", 0, 1, ids_buf(&[0, 1]), 0).await;
    assert!(resp.success);
    let out = resp.output_activation_tensor.expect("output");
    assert_eq!(out.shape, vec![2, common::VOCAB as i64]);
}

#[tokio::test]
async fn untied_full_range_forward_uses_output_weight() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(
        dir.path(),
        common::TinyModelConfig {
            n_layers: 1,
            tied: false,
            ..Default::default()
        },
    );
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 0, 0).await;
    let resp = forward(&mut client, "u1", 0, 0, ids_buf(&[1, 2]), 0).await;
    assert!(resp.success);
    let out = resp.output_activation_tensor.expect("output");
    assert_eq!(out.shape, vec![2, common::VOCAB as i64]);
}

// ---- the math gate -----------------------------------------------------------

async fn assert_single_block_matches_f64(qkv_bias: bool) {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(
        dir.path(),
        common::TinyModelConfig {
            n_layers: 1,
            tied: true,
            qkv_bias,
            ..Default::default()
        },
    );
    let mut client = connect(&fx).await;
    load_ok(&mut client, &fx, 0, 0).await;

    let resp = forward(&mut client, "ref", 0, 0, ids_buf(&[1, 2]), 0).await;
    assert!(resp.success);
    let out = out_f32(&resp.output_activation_tensor.expect("output"));

    // ---- f64 reference over the same fixture file ----
    let h = common::HIDDEN;
    let (heads, kv_heads, head_dim, n_ff, eps, theta) = (
        common::HEAD_COUNT,
        common::KV_HEAD_COUNT,
        common::HEAD_DIM,
        common::FEED_FORWARD,
        common::RMS_EPS as f64,
        common::ROPE_THETA as f64,
    );
    let file = ai_upcycler_worker::gguf::GgufFile::open(&fx.path).expect("fixture open");
    let f32_of = |name: &str| -> Vec<f64> {
        let desc = file.tensor(name).expect("tensor");
        assert_eq!(desc.ggml_type, 0, "fixture quant must be F32 for exactness");
        file.read_raw(name)
            .expect("read")
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c) as f64)
            .collect()
    };
    let mat = |name: &str| -> (Vec<f64>, usize, usize) {
        let desc = file.tensor(name).expect("tensor");
        // 1-D tensors have a single dim; the row count only matters for 2-D.
        (
            f32_of(name),
            desc.ne[0] as usize,
            desc.ne.get(1).copied().unwrap_or(1) as usize,
        )
    };

    let (token_embd, n_embd_rows, vocab) = mat("token_embd.weight");
    assert_eq!(n_embd_rows, h, "embedding row dim is ne[0]");
    let (attn_norm, _, _) = mat("blk.0.attn_norm.weight");
    let (att_qn, _, _) = mat("blk.0.attn_q_norm.weight");
    let (att_kn, _, _) = mat("blk.0.attn_k_norm.weight");
    let (wq, _, _) = mat("blk.0.attn_q.weight");
    let (wk, _, _) = mat("blk.0.attn_k.weight");
    let (wv, _, _) = mat("blk.0.attn_v.weight");
    let (woo, _, _) = mat("blk.0.attn_output.weight");
    let (ffn_norm, _, _) = mat("blk.0.ffn_norm.weight");
    let (wgate, _, _) = mat("blk.0.ffn_gate.weight");
    let (wup, _, _) = mat("blk.0.ffn_up.weight");
    let (wdown, _, _) = mat("blk.0.ffn_down.weight");
    let (out_norm, _, _) = mat("output_norm.weight");

    // Attention QKV biases exist only in the biased fixture variant.
    let bias1d = |name: &str| -> Option<Vec<f64>> {
        match file.tensor(name) {
            Some(desc) => {
                assert_eq!(desc.ggml_type, 0, "fixture biases must be F32");
                let v = f32_of(name);
                eprintln!("ref sees {name}: n={:} first={:?}", v.len(), v.first());
                Some(v)
            }
            None => {
                eprintln!("ref sees {name}: ABSENT");
                None
            }
        }
    };
    let (b_q, b_k, b_v) = (
        bias1d("blk.0.attn_q.bias"),
        bias1d("blk.0.attn_k.bias"),
        bias1d("blk.0.attn_v.bias"),
    );
    let add_bias = |add: &Option<Vec<f64>>, out: &mut Vec<f64>| {
        if let Some(b) = add {
            for (o, b) in out.iter_mut().zip(b) {
                *o += b;
            }
        }
    };

    let ids: [usize; 2] = [1, 2];
    assert_eq!(vocab, common::VOCAB);

    let rms = |v: &[f64], w: &[f64]| {
        let ms = v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64;
        let s = 1.0 / (ms + eps).sqrt();
        v.iter()
            .zip(w)
            .map(|(x, wi)| x * wi * s)
            .collect::<Vec<_>>()
    };
    let lin = |w: &[f64], n_in: usize, x: &[f64]| {
        (0..w.len() / n_in)
            .map(|o| (0..n_in).map(|i| w[o * n_in + i] * x[i]).sum::<f64>())
            .collect::<Vec<_>>()
    };

    // Token-by-token f64 reference with the same growing KV cache the engine
    // keeps: token t attends to rows 0..=t (causal), GQA head g reads
    // kv-head g / (heads/kv_heads).
    let scale = 1.0 / (head_dim as f64).sqrt();
    // growing per-kv-head cache: kv_rows[g] = [(k_row, v_row); rows so far]
    let mut kv_rows: Vec<Vec<(Vec<f64>, Vec<f64>)>> = vec![vec![]; kv_heads];
    let mut logits_ref: Vec<Vec<f64>> = Vec::with_capacity(ids.len());
    for (t, &id) in ids.iter().enumerate() {
        let pos = t; // sequence_position = 0
        let mut x: Vec<f64> = (0..h).map(|i| token_embd[id * h + i]).collect();

        // rms_norm(attn_norm) → q/k/v (+ attention bias when the fixture ships one)
        let xn = rms(&x, &attn_norm);
        let mut q = lin(&wq, h, &xn);
        let mut k = lin(&wk, h, &xn);
        let mut v = lin(&wv, h, &xn);
        add_bias(&b_q, &mut q);
        add_bias(&b_k, &mut k);
        add_bias(&b_v, &mut v);

        // per-head q_norm/k_norm
        for g in 0..heads {
            let (s, e) = (g * head_dim, (g + 1) * head_dim);
            let qh = rms(&q[s..e], &att_qn);
            q[s..e].copy_from_slice(&qh);
        }
        for g in 0..kv_heads {
            let (s, e) = (g * head_dim, (g + 1) * head_dim);
            let kh = rms(&k[s..e], &att_kn);
            k[s..e].copy_from_slice(&kh);
        }

        // rope on q and k — per head slice (head_dim), llama.cpp's NeoX
        // pairing: (j, j + head_dim/2), frequency index j
        let rope = |hc: &mut [f64]| {
            let half = hc.len() / 2;
            for j in 0..half {
                let angle = pos as f64 * theta.powf(-(2.0 * j as f64) / hc.len() as f64);
                let (c, s) = (angle.cos(), angle.sin());
                let (x0, x1) = (hc[j], hc[j + half]);
                hc[j] = x0 * c - x1 * s;
                hc[j + half] = x0 * s + x1 * c;
            }
        };
        for g in 0..heads {
            rope(&mut q[g * head_dim..(g + 1) * head_dim]);
        }
        for g in 0..kv_heads {
            rope(&mut k[g * head_dim..(g + 1) * head_dim]);
        }

        // append to the growing per-kv-head cache (persists across tokens)
        for g in 0..kv_heads {
            let krow = k[g * head_dim..(g + 1) * head_dim].to_vec();
            let vrow = v[g * head_dim..(g + 1) * head_dim].to_vec();
            kv_rows[g].push((krow, vrow));
        }

        // causal attention over rows 0..=t
        let mut attn = vec![0.0f64; h];
        for g in 0..heads {
            let h0 = g * head_dim;
            let kv_g = g / (heads / kv_heads);
            let q_head = &q[h0..h0 + head_dim];
            let scores: Vec<f64> = kv_rows[kv_g]
                .iter()
                .map(|(krow, _)| q_head.iter().zip(krow).map(|(a, b)| a * b).sum::<f64>() * scale)
                .collect();
            let max = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let exps: Vec<f64> = scores.iter().map(|s| (s - max).exp()).collect();
            let denom: f64 = exps.iter().sum();
            for (o, kv_pair) in kv_rows[kv_g].iter().enumerate() {
                let wgt = exps[o] / denom;
                let (_kr, vr) = kv_pair;
                for (ai, vi) in attn[h0..h0 + head_dim].iter_mut().zip(vr) {
                    *ai += vi * wgt;
                }
            }
        }

        // attn_output + residual
        let attn_out = lin(&woo, h, &attn);
        x.iter_mut().zip(&attn_out).for_each(|(a, o)| *a += o);

        // ffn: rms_norm → silu(gate)·up → down → residual
        let xn2 = rms(&x, &ffn_norm);
        let gate = lin(&wgate, h, &xn2);
        let up = lin(&wup, h, &xn2);
        let fused: Vec<f64> = gate
            .iter()
            .zip(&up)
            .map(|(g, u)| g / (1.0 + (-g).exp()) * u)
            .collect();
        let down = lin(&wdown, n_ff, &fused);
        x.iter_mut().zip(&down).for_each(|(a, o)| *a += o);

        // output_norm + tied head
        let normed = rms(&x, &out_norm);
        logits_ref.push(lin(&token_embd, h, &normed));
    }

    // engine output rows must match the reference within 1e-4 (plan gate);
    // global maxima printed for the PR evidence.
    let mut max_delta = 0.0f64;
    let mut max_logit = 0.0f64;
    for (t, want) in logits_ref.iter().enumerate() {
        for (i, &w) in want.iter().enumerate() {
            let got = out[t * common::VOCAB + i];
            max_delta = max_delta.max((got as f64 - w).abs());
            max_logit = max_logit.max(w.abs());
            assert!(
                (got as f64 - w).abs() <= 1e-4,
                "logit[{t}][{i}]: f32 {got} vs f64 {w} (Δ = {} > 1e-4)",
                (got as f64 - w).abs()
            );
        }
    }
    eprintln!("block-reference: max |logit| ≈ {max_logit:.4}, max f32−f64 Δ ≈ {max_delta:.2e}");
}

#[tokio::test]
async fn single_block_matches_f64_reference() {
    assert_single_block_matches_f64(false).await;
}

/// Qwen 0.5B-class files ship `blk.*.attn_{q,k,v}.bias`; the forward must add
/// them to the projections or real-model condensing diverges from llama.cpp.
#[tokio::test]
async fn qkv_bias_block_matches_f64_reference() {
    assert_single_block_matches_f64(true).await;
}
