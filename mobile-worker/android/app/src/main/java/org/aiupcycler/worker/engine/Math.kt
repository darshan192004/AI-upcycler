package org.aiupcycler.worker.engine

/**
 * Pure-F32 decoder math units: the primitive operations of the Qwen2 forward
 * pass, transcribed function-for-function from `worker/src/math.rs` (whose
 * references are ggml's `rms_norm` reduction / matmul accumulation, llama.cpp
 * RoPE pairing, softmax with max-subtraction, SiLU activation). Evaluation
 * order of the accumulate/normalize expressions matches the Rust exactly so
 * the deterministic multiply-add parts are bit-identical; the transcendental
 * calls (`sin`/`cos`/`exp`/`powf`) route through `java.lang.Math`, which can
 * differ from a Rust host libm by 1-2 ulp — the documented, bounded part of
 * the forward-parity contract (see the 2026-10-19 Android compute-RPCs spec,
 * D-A2).
 */

/**
 * RMS normalization in place: `x[i] = w[i] * x[i] / sqrt(mean(x^2) + eps)`.
 *
 * The mean of squares is computed over the whole slice **before** any
 * element is written, so the in-place update cannot observe its own
 * output. `weight.size` must equal `len`.
 */
internal fun rmsNorm(x: FloatArray, off: Int, len: Int, weight: FloatArray, eps: Float) {
    var meanSquare = 0.0f
    for (i in 0 until len) {
        val v = x[off + i]
        meanSquare += v * v
    }
    meanSquare /= len.toFloat()
    val scale = (1.0 / Math.sqrt((meanSquare + eps).toDouble())).toFloat()
    for (i in 0 until len) {
        x[off + i] *= weight[i] * scale
    }
}

/**
 * Dense mat-vec (`out[o] = sum_i w[o*nIn + i] * x[i]`, ggml row-major), over
 * writing `out`. `out.size` must be `weight.size / nIn`.
 */
internal fun linear(out: FloatArray, outOff: Int, outLen: Int, weight: FloatArray, nIn: Int, x: FloatArray, xOff: Int) {
    for (o in 0 until outLen) {
        val row = o * nIn
        var acc = 0.0f
        for (i in 0 until nIn) {
            acc += weight[row + i] * x[xOff + i]
        }
        out[outOff + o] = acc
    }
}

/**
 * Rotary-embedding rotation of one attention head in place, llama.cpp's
 * `GGML_ROPE_TYPE_NEOX` pairing (verified against `rotate_pairs` in
 * `ggml-cpu/ops.cpp`, commit d81235049): pair `j` (elements `j`,
 * `j + headDim/2`) rotates by `pos * theta^(-2j / headDim)` radians.
 * `len` must be even.
 */
internal fun ropeRow(head: FloatArray, off: Int, headDim: Int, pos: Int, theta: Float) {
    val half = headDim / 2
    for (j in 0 until half) {
        val angle = pos.toFloat() * Math.pow(
            theta.toDouble(),
            (-(2.0 * j) / headDim),
        ).toFloat()
        val sin = Math.sin(angle.toDouble()).toFloat()
        val cos = Math.cos(angle.toDouble()).toFloat()
        val x0 = head[off + j]
        val x1 = head[off + j + half]
        head[off + j] = x0 * cos - x1 * sin
        head[off + j + half] = x0 * sin + x1 * cos
    }
}

/** Softmax in place, max-subtracted for numerical stability. */
internal fun softmaxInplace(v: FloatArray, off: Int, len: Int) {
    var max = Float.NEGATIVE_INFINITY
    for (i in 0 until len) {
        max = Math.max(max, v[off + i])
    }
    var sum = 0.0f
    for (i in 0 until len) {
        val e = Math.exp(((v[off + i] - max)).toDouble()).toFloat()
        v[off + i] = e
        sum += e
    }
    if (sum > 0.0f) {
        for (i in 0 until len) {
            v[off + i] /= sum
        }
    }
}

/** SiLU activation in place: `v -> v * sigma(v)` with `sigma(v) = 1/(1 + e^-v)`. */
internal fun siluInplace(v: FloatArray, off: Int, len: Int) {
    for (i in 0 until len) {
        val e = v[off + i]
        v[off + i] = e / (1.0f + Math.exp((-e).toDouble()).toFloat())
    }
}
