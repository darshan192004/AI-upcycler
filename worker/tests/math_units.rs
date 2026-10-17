//! Decoder math unit tests (plan Task 5): every primitive is checked
//! against a test-side f64 reference (`|f32 − f64| ≤ 1e-5`) on inputs
//! including negatives and zeros, plus hand-computed literal cases where
//! noted in comments.

use ai_upcycler_worker::math::{linear, rms_norm, rope_row, silu_inplace, softmax_inplace};

// ----- f64 reference implementations (test-side) ---------------------------

fn ref_rms_norm(x: &[f64], w: &[f64], eps: f64) -> Vec<f64> {
    let ms = x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64;
    let scale = 1.0 / (ms + eps).sqrt();
    (0..x.len()).map(|i| w[i] * x[i] * scale).collect()
}

fn ref_linear(w: &[f64], n_in: usize, x: &[f64]) -> Vec<f64> {
    (0..w.len() / n_in)
        .map(|o| (0..n_in).map(|i| w[o * n_in + i] * x[i]).sum())
        .collect()
}

fn ref_rope(x: &[f64], pos: usize, theta: f64) -> Vec<f64> {
    let head_dim = x.len();
    (0..head_dim / 2)
        .flat_map(|j| {
            let angle = pos as f64 * theta.powf(-(2.0 * j as f64) / head_dim as f64);
            let (c, s) = (angle.cos(), angle.sin());
            [
                x[2 * j] * c - x[2 * j + 1] * s,
                x[2 * j] * s + x[2 * j + 1] * c,
            ]
        })
        .collect()
}

fn ref_softmax(v: &[f64]) -> Vec<f64> {
    let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let exps: Vec<f64> = v.iter().map(|x| (x - max).exp()).collect();
    let sum: f64 = exps.iter().sum();
    exps.iter().map(|e| e / sum).collect()
}

fn ref_silu(v: &[f64]) -> Vec<f64> {
    v.iter().map(|x| x / (1.0 + (-x).exp())).collect()
}

fn close(actual: &[f32], want: &[f64], tol: f64, what: &str) {
    assert_eq!(actual.len(), want.len(), "{what}: length");
    for (i, (a, b)) in actual.iter().zip(want).enumerate() {
        assert!(
            (*a as f64 - b).abs() <= tol,
            "{what}[{i}]: f32 {a} vs f64 {b} (Δ = {}, tol {tol})",
            (*a as f64 - b).abs()
        );
    }
}

// ----- rms_norm ------------------------------------------------------------

#[test]
fn rms_norm_matches_f64_reference() {
    let eps = 1e-6f32;
    let cases: Vec<Vec<f32>> = vec![
        // negatives, zeros, mixed magnitudes
        vec![1.5, -2.0, 0.0, 3.25],
        vec![-0.75, 0.0, 0.0, 1.0],
        vec![100.0, -100.0, 0.5, -0.25],
    ];
    let w = [1.1f32, 0.9, 1.0, 0.8];
    for (n, x) in cases.iter().enumerate() {
        let mut got = x.clone();
        rms_norm(&mut got, &w, eps);
        close(
            &got,
            &ref_rms_norm(
                &x.iter().map(|v| *v as f64).collect::<Vec<_>>(),
                &w.iter().map(|v| *v as f64).collect::<Vec<_>>(),
                eps as f64,
            ),
            1e-5,
            &format!("rms_norm case {n}"),
        );
    }
}

#[test]
fn rms_norm_literal_case() {
    // x = [3, 4], w = [1, 1], eps = 1e-12 → [3, 4] / sqrt((9 + 16)/2 + 1e-12)
    // mean square = 12.5 → scale = 1/√12.5 = 0.282842712…
    // expected: [3/√12.5, 4/√12.5] = [0.8485281374, 1.1313708499]
    let mut x = [3.0f32, 4.0];
    rms_norm(&mut x, &[1.0f32, 1.0], 1e-12);
    let sqrt12_5 = 12.5f64.sqrt();
    close(&x, &[3.0 / sqrt12_5, 4.0 / sqrt12_5], 1e-6, "rms literal");
}

// ----- linear --------------------------------------------------------------

#[test]
fn linear_matches_f64_reference() {
    // w rows: [1, -2, 3, 0] and [-1, 4, -0.5, 2]
    let w = [1.0f32, -2.0, 3.0, 0.0, -1.0, 4.0, -0.5, 2.0];
    let x = [0.5f32, -1.25, 2.0, 0.0];
    let mut out = [0.0f32; 2];
    linear(&mut out, &w, 4, &x);
    close(
        &out,
        &ref_linear(
            &w.iter().map(|v| *v as f64).collect::<Vec<_>>(),
            4,
            &x.iter().map(|v| *v as f64).collect::<Vec<_>>(),
        ),
        1e-5,
        "linear ref",
    );
}

#[test]
fn linear_2x3_known_matrix() {
    // w = [[1, 2, 3], [4, 5, 6]], x = [2, 0, 1] (row-major w[o·n_in + i]):
    //   out[0] = 1·2 + 2·0 + 3·1 = 5
    //   out[1] = 4·2 + 5·0 + 6·1 = 14
    let mut out = [0.0f32; 2];
    linear(
        &mut out,
        &[1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0],
        3,
        &[2.0, 0.0, 1.0],
    );
    close(&out, &[5.0, 14.0], 1e-6, "linear literal");
}

// ----- rope_row ------------------------------------------------------------

#[test]
fn rope_pos0_is_identity() {
    // pos = 0 → angle 0 for every pair → cos = 1, sin = 0 → bit-exact identity.
    let x = [1.0f32, -2.0, 3.0, -0.5, 0.25, 7.0];
    let mut got = x;
    rope_row(&mut got, 0, 10000.0);
    assert_eq!(got, x);
}

#[test]
fn rope_preserves_pair_norm() {
    // Rotations preserve each pair's squared norm (property, pos 1..=3).
    let x = [1.0f32, -2.0, 3.0, -0.5, 0.25, 7.0];
    let pair_norm = |v: &[f32; 6]| {
        (0..3)
            .map(|j| (v[2 * j] as f64).powi(2) + (v[2 * j + 1] as f64).powi(2))
            .collect::<Vec<f64>>()
    };
    let before = pair_norm(&x);
    for pos in 1..=3 {
        let mut got = x;
        rope_row(&mut got, pos, 10000.0);
        let after = pair_norm(&got);
        for (i, (b, a)) in before.iter().zip(&after).enumerate() {
            assert!((b - a).abs() <= 1e-5, "pair {i} norm pos {pos}: {b} → {a}");
        }
    }
}

#[test]
fn rope_matches_f64_reference() {
    // pos = 1..4 against theta = 10000 (the worker's qwen2 default).
    let x = [1.0f32, -2.0, 3.0, -0.5, 0.25, 7.0, 11.0, -13.0];
    let x64 = x.iter().map(|v| *v as f64).collect::<Vec<_>>();
    for pos in 1..=4 {
        let mut got = x;
        rope_row(&mut got, pos, 10000.0);
        close(
            &got,
            &ref_rope(&x64, pos, 10000.0),
            1e-5,
            &format!("rope pos {pos}"),
        );
    }
}

// ----- softmax -------------------------------------------------------------

#[test]
fn softmax_matches_f64_reference() {
    let v = [1.0f32, 2.0, 0.5, -3.0];
    let mut got = v;
    softmax_inplace(&mut got);
    close(
        &got,
        &ref_softmax(&v.iter().map(|x| *x as f64).collect::<Vec<_>>()),
        1e-5,
        "softmax ref",
    );
}

#[test]
fn softmax_literal_case() {
    // [0, ln 2] → exp = [1, 2] → sums 3 → [1/3, 2/3]
    let mut v = [0.0f32, core::f32::consts::LN_2];
    softmax_inplace(&mut v);
    close(&v, &[1.0 / 3.0, 2.0 / 3.0], 1e-6, "softmax literal");
}

// ----- silu ----------------------------------------------------------------

#[test]
fn silu_matches_f64_reference() {
    let v = [-2.0f32, -0.5, 0.0, 1.5];
    let mut got = v;
    silu_inplace(&mut got);
    close(
        &got,
        &ref_silu(&v.iter().map(|x| *x as f64).collect::<Vec<_>>()),
        1e-5,
        "silu ref",
    );
}

#[test]
fn silu_literal_case() {
    // σ(-2) = 1/(1+e²) = 0.1192029220…  → silu(-2) = -0.2384058441…
    // σ(1.5) = 1/(1+e⁻¹·⁵) = 0.8175744762… → silu(1.5) = 1.22636171…
    // σ(0) = 0.5 → silu(0) = 0
    let mut v = [-2.0f32, 1.5, 0.0];
    silu_inplace(&mut v);
    close(&v, &[-0.238405844, 1.22636172, 0.0], 1e-6, "silu literal");
}
