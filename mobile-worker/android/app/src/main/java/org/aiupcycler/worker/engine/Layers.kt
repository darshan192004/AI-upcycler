package org.aiupcycler.worker.engine

/**
 * Real decoder layers: per-layer KV cache, weight blocks, and the qwen2 block
 * pass, transcribed function-for-function from `worker/src/layers.rs`. All
 * math is F32 through [rmsNorm]/[linear]/[ropeRow]/[softmaxInplace]/[siluInplace].
 */

/**
 * Per-layer K/V cache: rows of `[kv_heads * head_dim]`, appended one token at
 * a time (row-major). Grows by doubling; never allocates more than the
 * model's context length rows (the engine validates `position + n <= ctx`).
 */
internal class KvLayer(private val kvDim: Int, private val capRows: Int) {
    private var k = FloatArray(0)
    private var v = FloatArray(0)
    var lenRows = 0
        private set

    val isEmpty: Boolean
        get() = lenRows == 0

    /** Pushes one row pair (k must already be RoPE-rotated in [runBlock]). */
    fun append(kRow: FloatArray, kOff: Int, vRow: FloatArray, vOff: Int) {
        assert(lenRows < capRows) { "KV over context length" }
        val rows = k.size / kvDim
        if (lenRows == rows) {
            // Doubling growth, clamped to the context cap.
            val grow = maxOf(rows, 1).coerceAtMost(capRows - lenRows)
            k = k.copyOf((rows + grow) * kvDim)
            v = v.copyOf((rows + grow) * kvDim)
        }
        val o = lenRows * kvDim
        System.arraycopy(kRow, kOff, k, o, kvDim)
        System.arraycopy(vRow, vOff, v, o, kvDim)
        lenRows++
    }

    /** Copies key row `r` into `out` at `outOff`. */
    fun copyKRow(r: Int, out: FloatArray, outOff: Int) {
        System.arraycopy(k, r * kvDim, out, outOff, kvDim)
    }

    /** Copies value row `r` into `out` at `outOff`. */
    fun copyVRow(r: Int, out: FloatArray, outOff: Int) {
        System.arraycopy(v, r * kvDim, out, outOff, kvDim)
    }
}

/**
 * The materialized F32 tensors of one block, in forward-pass shape.
 * `qNorm`/`kNorm` are absent in real qwen2 files (QK-norm is qwen3-style);
 * [runBlock] skips them when `null`. The Qwen 0.5B-class attention biases,
 * when present, are added to the projections.
 */
internal class BlockWeights private constructor(
    val attnNorm: FloatArray,
    val ffnNorm: FloatArray,
    val qNorm: FloatArray?,
    val kNorm: FloatArray?,
    val attnQ: FloatArray,
    val attnK: FloatArray,
    val attnV: FloatArray,
    val attnOutput: FloatArray,
    val qBias: FloatArray?,
    val kBias: FloatArray?,
    val vBias: FloatArray?,
    val ffnGate: FloatArray,
    val ffnUp: FloatArray,
    val ffnDown: FloatArray,
) {
    companion object {
        /**
         * Takes the block's tensors out of the loaded map (the forward owns
         * them after extraction). Required tensors are contract-backed:
         * `LoadLayers` refuses missing ones, so this cannot fire mislabeled.
         */
        fun take(tensors: MutableMap<String, FloatArray>, layer: Int): BlockWeights {
            fun name(part: String) = "blk.$layer.$part"
            fun req(part: String) =
                tensors.remove(name(part)) ?: throw IllegalArgumentException("$part loaded")
            return BlockWeights(
                attnNorm = req("attn_norm.weight"),
                ffnNorm = req("ffn_norm.weight"),
                qNorm = tensors.remove(name("attn_q_norm.weight")),
                kNorm = tensors.remove(name("attn_k_norm.weight")),
                attnQ = req("attn_q.weight"),
                attnK = req("attn_k.weight"),
                attnV = req("attn_v.weight"),
                attnOutput = req("attn_output.weight"),
                qBias = tensors.remove(name("attn_q.bias")),
                kBias = tensors.remove(name("attn_k.bias")),
                vBias = tensors.remove(name("attn_v.bias")),
                ffnGate = req("ffn_gate.weight"),
                ffnUp = req("ffn_up.weight"),
                ffnDown = req("ffn_down.weight"),
            )
        }

        /** Puts the block's tensors back (the inverse of [take]) so later forwards share allocations. */
        fun putBack(tensors: MutableMap<String, FloatArray>, layer: Int, w: BlockWeights) {
            fun name(part: String) = "blk.$layer.$part"
            tensors[name("attn_norm.weight")] = w.attnNorm
            tensors[name("ffn_norm.weight")] = w.ffnNorm
            w.qNorm?.let { tensors[name("attn_q_norm.weight")] = it }
            w.kNorm?.let { tensors[name("attn_k_norm.weight")] = it }
            w.qBias?.let { tensors[name("attn_q.bias")] = it }
            w.kBias?.let { tensors[name("attn_k.bias")] = it }
            w.vBias?.let { tensors[name("attn_v.bias")] = it }
            tensors[name("attn_q.weight")] = w.attnQ
            tensors[name("attn_k.weight")] = w.attnK
            tensors[name("attn_v.weight")] = w.attnV
            tensors[name("attn_output.weight")] = w.attnOutput
            tensors[name("ffn_gate.weight")] = w.ffnGate
            tensors[name("ffn_up.weight")] = w.ffnUp
            tensors[name("ffn_down.weight")] = w.ffnDown
        }
    }
}

/** Token embedding lookup: `out[t*hidden + i] = tokenEmbd[id_t*hidden + i]`. */
internal fun embedIds(ids: IntArray, tokenEmbd: FloatArray, hidden: Int, out: FloatArray) {
    for (t in ids.indices) {
        val src = ids[t] * hidden
        System.arraycopy(tokenEmbd, src, out, t * hidden, hidden)
    }
}

/**
 * LM head: `out[o] = sum_i head[o*hidden + i] * row[i]`. The head tensor's
 * orientation equals `token_embd`'s (`ne[0] = hidden`), so a tied model
 * reuses the embedding as the head directly. One call writes `outLen` values
 * at `outOff` — one logits row.
 */
internal fun lmHead(
    hiddenRow: FloatArray,
    rowOff: Int,
    head: FloatArray,
    hidden: Int,
    out: FloatArray,
    outOff: Int,
    outLen: Int,
) {
    linear(out, outOff, outLen, head, hidden, hiddenRow, rowOff)
}

/**
 * One qwen2 block over `nTokens` rows of `x` (`[n][hidden]`), through both
 * residuals in place. `positions` start = absolute position of row 0. Appends
 * each token's rotated k/v to `kv`; attention reads the whole cache
 * (row `t` sees keys `0..prevLen + t + 1`).
 */
internal fun runBlock(
    w: BlockWeights,
    h: ModelHparams,
    kv: KvLayer,
    x: FloatArray,
    nTokens: Int,
    positionsStart: Int,
) {
    val hidden = h.embeddingLength
    val heads = h.headCount
    val kvHeads = h.headCountKv
    val headDim = h.headDim
    val nFf = h.feedForwardLength
    val qDim = heads * headDim
    val kvDim = kvHeads * headDim
    val prevLen = kv.lenRows
    assert(heads * headDim == hidden) {
        "qwen2 attention width equals hidden (qwen3 MHA-variant may differ)"
    }

    val normed = FloatArray(hidden)
    val qAll = FloatArray(nTokens * qDim)
    val kAll = FloatArray(nTokens * kvDim)
    val vAll = FloatArray(nTokens * kvDim)

    // 1. projections (+ optional qk-norm, rope, cache append) per row.
    for (t in 0 until nTokens) {
        val row = t * hidden
        System.arraycopy(x, row, normed, 0, hidden)
        rmsNorm(normed, 0, hidden, w.attnNorm, h.rmsEps)

        val q = t * qDim
        val ks = t * kvDim
        val vs = t * kvDim
        linear(qAll, q, qDim, w.attnQ, hidden, normed, 0)
        linear(kAll, ks, kvDim, w.attnK, hidden, normed, 0)
        linear(vAll, vs, kvDim, w.attnV, hidden, normed, 0)
        w.qBias?.let { b -> for (o in 0 until qDim) qAll[q + o] += b[o] }
        w.kBias?.let { b -> for (o in 0 until kvDim) kAll[ks + o] += b[o] }
        w.vBias?.let { b -> for (o in 0 until kvDim) vAll[vs + o] += b[o] }

        w.qNorm?.let { qn ->
            for (g in 0 until heads) {
                val s = q + g * headDim
                rmsNorm(qAll, s, headDim, qn, h.rmsEps)
            }
        }
        w.kNorm?.let { kn ->
            for (g in 0 until kvHeads) {
                val s = ks + g * headDim
                rmsNorm(kAll, s, headDim, kn, h.rmsEps)
            }
        }

        val pos = positionsStart + t
        for (g in 0 until heads) {
            val s = q + g * headDim
            ropeRow(qAll, s, headDim, pos, h.ropeTheta)
        }
        for (g in 0 until kvHeads) {
            val s = ks + g * headDim
            ropeRow(kAll, s, headDim, pos, h.ropeTheta)
        }

        kv.append(kAll, ks, vAll, vs)
    }

    // 2. causal attention per token, then attn_output + residual. GQA groups
    // are contiguous: head `gh` reads kv-head `gh / (heads / kvHeads)`.
    val ratio = heads / kvHeads
    val scale = (1.0 / Math.sqrt(headDim.toDouble())).toFloat()
    val scores = FloatArray(prevLen + nTokens)
    val attRow = FloatArray(hidden)
    val headOut = FloatArray(headDim)
    val kRow = FloatArray(kvDim)
    val vRow = FloatArray(kvDim)
    val projected = FloatArray(hidden)
    for (t in 0 until nTokens) {
        attRow.fill(0.0f)
        val visible = prevLen + t + 1
        for (gh in 0 until heads) {
            val q0 = t * qDim + gh * headDim
            val kvHeadIdx = gh / ratio
            scores.fill(0.0f, 0, visible)
            for (r in 0 until visible) {
                var score = 0.0f
                // The Rust code advances `score` in one fused multiply-add
                // loop against the cached key row; the copy-then-accumulate
                // below reads the same elements in the same order.
                kv.copyKRow(r, kRow, 0)
                val ks = kvHeadIdx * headDim
                for (qi in 0 until headDim) {
                    score += qAll[q0 + qi] * kRow[ks + qi]
                }
                scores[r] = score * scale
            }
            softmaxInplace(scores, 0, visible)
            val hs = kvHeadIdx * headDim
            headOut.fill(0.0f)
            for (r in 0 until visible) {
                val weight = scores[r]
                kv.copyVRow(r, vRow, 0)
                for (hi in 0 until headDim) {
                    headOut[hi] += weight * vRow[hs + hi]
                }
            }
            System.arraycopy(headOut, 0, attRow, gh * headDim, headDim)
        }
        linear(projected, 0, hidden, w.attnOutput, hidden, attRow, 0)
        for (i in 0 until hidden) {
            x[t * hidden + i] += projected[i]
        }
    }

    // 3. FFN per row: rms_norm -> silu(gate)*up -> down -> residual.
    val gate = FloatArray(nFf)
    val up = FloatArray(nFf)
    val down = FloatArray(hidden)
    for (t in 0 until nTokens) {
        val row = t * hidden
        System.arraycopy(x, row, normed, 0, hidden)
        rmsNorm(normed, 0, hidden, w.ffnNorm, h.rmsEps)
        linear(gate, 0, nFf, w.ffnGate, hidden, normed, 0)
        linear(up, 0, nFf, w.ffnUp, hidden, normed, 0)
        siluInplace(gate, 0, nFf)
        for (g in 0 until nFf) {
            gate[g] *= up[g]
        }
        linear(down, 0, hidden, w.ffnDown, nFf, gate, 0)
        for (i in 0 until hidden) {
            x[row + i] += down[i]
        }
    }
}
