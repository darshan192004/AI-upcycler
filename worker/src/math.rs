//! Pure-F32 decoder math units (plan Task 5): the primitive operations of
//! the Qwen2 forward pass, each a direct transcription of its ggml
//! reference (`ggml.c` `ggml_rms_norm` reduction / `ggml_tensor` matmul
//! accumulation, `ggml-cuda`/`llama.cpp` RoPE pairing, softmax with
//! max-subtraction, SiLU activation). No allocation, no hidden state.

/// RMS normalization in place: `x[i] = w[i] Â· x[i] / sqrt(mean(xÂ²) + eps)`.
///
/// The mean of squares is computed over the whole slice **before** any
/// element is written, so the in-place update cannot observe its own
/// output. `weight.len()` must equal `x.len()`.
pub fn rms_norm(x: &mut [f32], weight: &[f32], eps: f32) {
    debug_assert_eq!(weight.len(), x.len(), "rms_norm weight/x length");
    let mean_square = x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32;
    let scale = 1.0 / (mean_square + eps).sqrt();
    for (v, w) in x.iter_mut().zip(weight) {
        *v *= w * scale;
    }
}

/// Dense mat-vec (`out[o] = Î£áµ¢ w[oÂ·n_in + i] Â· x[i]`, ggml row-major
/// `w[oÂ·n_in + i]`), overwriting `out`. `out.len()` must be
/// `weight.len() / n_in`.
pub fn linear(out: &mut [f32], weight: &[f32], n_in: usize, x: &[f32]) {
    debug_assert!(n_in > 0, "linear n_in");
    debug_assert_eq!(weight.len(), out.len() * n_in, "linear weight shape");
    debug_assert_eq!(x.len(), n_in, "linear x/n_in length");
    for (o, out_v) in out.iter_mut().enumerate() {
        let row = &weight[o * n_in..(o + 1) * n_in];
        let acc = row.iter().zip(x).map(|(w, xv)| w * xv).sum::<f32>();
        *out_v = acc;
    }
}

/// Rotary-embedding rotation of one attention head in place, llama.cpp's
/// `GGML_ROPE_TYPE_NEOX` pairing (verified against `rotate_pairs` in
/// `ggml-cpu/ops.cpp`, commit d81235049): pair `j` (elements `j`,
/// `j + head_dim/2`) rotates by `pos · theta^(-2j / head_dim)` radians.
/// `head.len()` must be even.
pub fn rope_row(head: &mut [f32], pos: usize, theta: f32) {
    let head_dim = head.len();
    debug_assert!(head_dim.is_multiple_of(2), "rope head_dim must be even");
    let half = head_dim / 2;
    for j in 0..half {
        let angle = pos as f32 * theta.powf(-(2.0 * j as f32) / head_dim as f32);
        let (sin, cos) = angle.sin_cos();
        let (x0, x1) = (head[j], head[j + half]);
        head[j] = x0 * cos - x1 * sin;
        head[j + half] = x0 * sin + x1 * cos;
    }
}

/// Softmax in place, max-subtracted for numerical stability.
pub fn softmax_inplace(v: &mut [f32]) {
    let max = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0f32;
    for e in v.iter_mut() {
        *e = (*e - max).exp();
        sum += *e;
    }
    if sum > 0.0 {
        for e in v.iter_mut() {
            *e /= sum;
        }
    }
}

/// SiLU activation in place: `v â†’ v Â· Ïƒ(v)` with `Ïƒ(v) = 1/(1 + e^-v)`.
pub fn silu_inplace(v: &mut [f32]) {
    for e in v.iter_mut() {
        *e /= 1.0 + (-*e).exp();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softmax_sums_to_one_even_if_all_equal() {
        let mut v = [2.0f32; 4];
        softmax_inplace(&mut v);
        assert_eq!(v, [0.25; 4]);
    }
}
