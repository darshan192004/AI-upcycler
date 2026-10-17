//! Real decoder layers (plan Task 6): per-layer KV cache, weight blocks,
//! and the qwen2 block pass. All math is F32 through `math::*`.

use crate::gguf::ModelHparams;
use crate::math::{linear, rms_norm, rope_row, silu_inplace, softmax_inplace};

/// Per-layer K/V cache: rows of `[kv_heads · head_dim]`, appended one token
/// at a time (row-major). Grows by doubling; never allocates more than the
/// model's context length rows (the engine validates `position + n <= ctx`).
pub struct KvLayer {
    kv_dim: usize,
    cap_rows: usize,
    len_rows: usize,
    k: Vec<f32>,
    v: Vec<f32>,
}

impl KvLayer {
    pub fn new(kv_dim: usize, cap_rows: usize) -> Self {
        debug_assert!(kv_dim > 0 && cap_rows > 0, "KvLayer dims");
        Self {
            kv_dim,
            cap_rows,
            len_rows: 0,
            k: Vec::new(),
            v: Vec::new(),
        }
    }

    /// Number of cached rows (tokens); `is_empty()` complements `len()`.
    pub fn len(&self) -> usize {
        self.len_rows
    }

    /// True while the cache holds no rows.
    pub fn is_empty(&self) -> bool {
        self.len_rows == 0
    }

    /// Pushes one row pair (k must already be RoPE-rotated in `run_block`).
    pub fn append(&mut self, k_row: &[f32], v_row: &[f32]) {
        debug_assert_eq!(k_row.len(), self.kv_dim);
        debug_assert!(self.len_rows < self.cap_rows, "KV over context length");
        let rows = self.k.len() / self.kv_dim;
        if self.len_rows == rows {
            // Doubling growth, clamped to the context cap.
            let grow = rows.max(1).min(self.cap_rows - self.len_rows);
            self.k.resize((rows + grow) * self.kv_dim, 0.0);
            self.v.resize((rows + grow) * self.kv_dim, 0.0);
        }
        let o = self.len_rows * self.kv_dim;
        self.k[o..o + self.kv_dim].copy_from_slice(k_row);
        self.v[o..o + self.kv_dim].copy_from_slice(v_row);
        self.len_rows += 1;
    }

    /// Key row `r`.
    pub fn k_row(&self, r: usize) -> &[f32] {
        &self.k[r * self.kv_dim..(r + 1) * self.kv_dim]
    }

    /// Value row `r`.
    pub fn v_row(&self, r: usize) -> &[f32] {
        &self.v[r * self.kv_dim..(r + 1) * self.kv_dim]
    }
}

/// The materialized F32 tensors of one block, in forward-pass shape.
/// `q_norm`/`k_norm` are absent in real qwen2 files (QK-norm is qwen3-style);
/// `run_block` skips them when `None`.
pub struct BlockWeights {
    pub attn_norm: Vec<f32>,
    pub ffn_norm: Vec<f32>,
    pub q_norm: Option<Vec<f32>>,
    pub k_norm: Option<Vec<f32>>,
    pub attn_q: Vec<f32>,
    pub attn_k: Vec<f32>,
    pub attn_v: Vec<f32>,
    pub attn_output: Vec<f32>,
    pub ffn_gate: Vec<f32>,
    pub ffn_up: Vec<f32>,
    pub ffn_down: Vec<f32>,
}

impl BlockWeights {
    /// Takes the block's tensors out of the loaded map (the forward owns
    /// them after extraction). Required tensors are contract-backed:
    /// `LoadLayers` refuses missing ones, so `expect` cannot fire.
    pub fn take(tensors: &mut std::collections::HashMap<String, Vec<f32>>, layer: u32) -> Self {
        let name = |part: &str| format!("blk.{layer}.{part}");
        Self {
            attn_norm: tensors
                .remove(&name("attn_norm.weight"))
                .expect("attn_norm loaded"),
            ffn_norm: tensors
                .remove(&name("ffn_norm.weight"))
                .expect("ffn_norm loaded"),
            q_norm: tensors.remove(&name("attn_q_norm.weight")),
            k_norm: tensors.remove(&name("attn_k_norm.weight")),
            attn_q: tensors
                .remove(&name("attn_q.weight"))
                .expect("attn_q loaded"),
            attn_k: tensors
                .remove(&name("attn_k.weight"))
                .expect("attn_k loaded"),
            attn_v: tensors
                .remove(&name("attn_v.weight"))
                .expect("attn_v loaded"),
            attn_output: tensors
                .remove(&name("attn_output.weight"))
                .expect("attn_output loaded"),
            ffn_gate: tensors
                .remove(&name("ffn_gate.weight"))
                .expect("ffn_gate loaded"),
            ffn_up: tensors
                .remove(&name("ffn_up.weight"))
                .expect("ffn_up loaded"),
            ffn_down: tensors
                .remove(&name("ffn_down.weight"))
                .expect("ffn_down loaded"),
        }
    }
}

/// Token embedding lookup: `out[t·hidden + i] = token_embd[id_t·hidden + i]`.
pub fn embed_ids(ids: &[u32], token_embd: &[f32], hidden: usize, out: &mut [f32]) {
    debug_assert_eq!(ids.len() * hidden, out.len());
    for (t, &id) in ids.iter().enumerate() {
        let src = id as usize * hidden;
        out[t * hidden..(t + 1) * hidden].copy_from_slice(&token_embd[src..src + hidden]);
    }
}

/// LM head: `out[o] = Σᵢ head[o·hidden + i] · row[i]`. The head tensor's
/// orientation equals `token_embd`'s (`ne[0] = hidden`), so a tied model
/// reuses the embedding as the head directly.
pub fn lm_head(hidden_row: &[f32], head: &[f32], hidden: usize, out: &mut [f32]) {
    linear(out, head, hidden, hidden_row);
}

/// One qwen2 block over `n_tokens` rows of `x` (`[n][hidden]`), through both
/// residuals in place. `positions` = absolute positions of each row. Appends
/// each token's rotated k/v to `kv`; attention reads the whole cache
/// (row `t` sees keys `0..prev_len + t + 1`).
pub fn run_block(
    w: &BlockWeights,
    h: &ModelHparams,
    kv: &mut KvLayer,
    x: &mut [f32],
    n_tokens: usize,
    positions: std::ops::Range<usize>,
) {
    let hidden = h.embedding_length as usize;
    let heads = h.head_count as usize;
    let kv_heads = h.head_count_kv as usize;
    let head_dim = h.head_dim as usize;
    let n_ff = h.feed_forward_length as usize;
    let q_dim = heads * head_dim;
    let kv_dim = kv_heads * head_dim;
    let prev_len = kv.len();
    debug_assert_eq!(
        heads * head_dim,
        hidden,
        "qwen2 attention width equals hidden (qwen3 MHA-variant may differ)"
    );

    let mut normed = vec![0.0f32; hidden];
    let mut q_all = vec![0.0f32; n_tokens * q_dim];
    let mut k_all = vec![0.0f32; n_tokens * kv_dim];
    let mut v_all = vec![0.0f32; n_tokens * kv_dim];

    // 1. projections (+ optional qk-norm, rope, cache append) per row.
    for t in 0..n_tokens {
        let row = t * hidden;
        normed.copy_from_slice(&x[row..row + hidden]);
        rms_norm(&mut normed, &w.attn_norm, h.rms_eps);

        let (q, ks, vs) = (t * q_dim, t * kv_dim, t * kv_dim);
        linear(&mut q_all[q..q + q_dim], &w.attn_q, hidden, &normed);
        linear(&mut k_all[ks..ks + kv_dim], &w.attn_k, hidden, &normed);
        linear(&mut v_all[vs..vs + kv_dim], &w.attn_v, hidden, &normed);

        if let Some(qn) = &w.q_norm {
            for g in 0..heads {
                let s = q + g * head_dim;
                rms_norm(&mut q_all[s..s + head_dim], qn, h.rms_eps);
            }
        }
        if let Some(kn) = &w.k_norm {
            for g in 0..kv_heads {
                let s = ks + g * head_dim;
                rms_norm(&mut k_all[s..s + head_dim], kn, h.rms_eps);
            }
        }

        let pos = positions.start + t;
        for g in 0..heads {
            let s = q + g * head_dim;
            rope_row(&mut q_all[s..s + head_dim], pos, h.rope_theta);
        }
        for g in 0..kv_heads {
            let s = ks + g * head_dim;
            rope_row(&mut k_all[s..s + head_dim], pos, h.rope_theta);
        }

        kv.append(&k_all[ks..ks + kv_dim], &v_all[vs..vs + kv_dim]);
    }

    // 2. causal attention per token, then attn_output + residual. GQA groups
    // are contiguous: head `gh` reads kv-head `gh / (heads / kv_heads)`.
    let ratio = heads / kv_heads;
    let scale = 1.0 / (head_dim as f32).sqrt();
    let mut scores = vec![0.0f32; prev_len + n_tokens];
    let mut att_row = vec![0.0f32; hidden];
    let mut head_out = vec![0.0f32; head_dim];
    let mut projected = vec![0.0f32; hidden];
    for t in 0..n_tokens {
        att_row.fill(0.0);
        let visible = prev_len + t + 1;
        for gh in 0..heads {
            let q0 = t * q_dim + gh * head_dim;
            let kv_head = gh / ratio;
            scores[..visible].fill(0.0);
            for (r, score) in scores[..visible].iter_mut().enumerate() {
                let k_row = kv.k_row(r);
                let ks = kv_head * head_dim;
                for (qi, &qv) in q_all[q0..q0 + head_dim].iter().enumerate() {
                    *score += qv * k_row[ks + qi];
                }
                *score *= scale;
            }
            softmax_inplace(&mut scores[..visible]);
            let hs = kv_head * head_dim;
            head_out.fill(0.0);
            for (r, &weight) in scores[..visible].iter().enumerate() {
                let v_row = kv.v_row(r);
                for (hi, ho) in head_out.iter_mut().enumerate() {
                    *ho += weight * v_row[hs + hi];
                }
            }
            att_row[gh * head_dim..gh * head_dim + head_dim].copy_from_slice(&head_out);
        }
        linear(&mut projected, &w.attn_output, hidden, &att_row);
        for i in 0..hidden {
            x[t * hidden + i] += projected[i];
        }
    }

    // 3. FFN per row: rms_norm → silu(gate)·up → down → residual.
    let mut gate = vec![0.0f32; n_ff];
    let mut up = vec![0.0f32; n_ff];
    let mut down = vec![0.0f32; hidden];
    for t in 0..n_tokens {
        let row = t * hidden;
        normed.copy_from_slice(&x[row..row + hidden]);
        rms_norm(&mut normed, &w.ffn_norm, h.rms_eps);
        linear(&mut gate, &w.ffn_gate, hidden, &normed);
        linear(&mut up, &w.ffn_up, hidden, &normed);
        silu_inplace(&mut gate);
        for (g, u) in gate.iter_mut().zip(&up) {
            *g *= u;
        }
        linear(&mut down, &w.ffn_down, n_ff, &gate);
        for i in 0..hidden {
            x[row + i] += down[i];
        }
    }
}
