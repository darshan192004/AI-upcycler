package org.aiupcycler.worker.engine

import java.io.File
import org.aiupcycler.worker.rpc.EngineSource

/**
 * Worker engine state machine behind the `WorkerService` compute RPCs —
 * `LoadLayers` retains materialized F32 weights for the requested range,
 * `ExecuteForward` runs the real qwen2 block pass with per-session KV caches,
 * `UnloadLayers` frees them. This is the Kotlin mirror of
 * `worker/src/engine.rs` (the reference engine): validation order and every
 * refusal condition match so the master sees one consistent contract on both
 * worker kinds. Error messages mirror the Rust wording the master surfaces.
 */

/** Load/forward outcome (`success:false` refusals on the wire). */
sealed class EngineOutcome

class LoadFailure(val message: String) : EngineOutcome()

class LoadSuccess(
    val allocatedRamBytes: Long,
    val startLayer: Int,
    val endLayer: Int,
) : EngineOutcome()

class UnloadFailure(val message: String) : EngineOutcome()

class UnloadSuccess(val freedRamBytes: Long) : EngineOutcome()

class ForwardFailure(val reason: String) : EngineOutcome()

class ForwardResult(
    val shapeOut: LongArray,
    val out: FloatArray,
    /** The caller owns timing; the engine surfaces it separately. See [Engine.execute]. */
    val isHead: Boolean,
) : EngineOutcome()

/**
 * One engine: holds the parsed model file, the loaded layer range and the
 * per-session forward state. Construction parses and validates the GGUF —
 * a file that fails validation throws and the caller must not wire the engine
 * into the RPC service (the local Rust binary exits 1 before announcing READY;
 * the phone equivalent is to serve metadata-only with compute `UNIMPLEMENTED`).
 *
 * Implements `EngineSource` (the RPC layer's seam, spec 2026-10-19
 * android-compute-rpcs): the three compute RPCs delegate straight through,
 * so validation order, refusals and byte accounting are the wire contract
 * without a translation layer.
 *
 * Concurrency: every state-touching entry point (`loadLayers`, `execute`,
 * `unloadLayers`, `close`) is `@Synchronized` - one monitor guarding
 * `loaded` + `sessions` for the whole call, the exact shape of the Rust
 * reference's `Arc<Mutex<EngineState>>` with one `MutexGuard` per RPC
 * (engine.rs). gRPC dispatches on a thread pool; the in-process test
 * transport using `directExecutor` does not remove that.
 */
class WorkerEngine private constructor(
    private val model: GgufFile,
    val modelName: String,
) : EngineSource {
    private var loaded: LoadedRange? = null
    private val sessions = HashMap<String, Session>()

    private class LoadedRange(
        val start: Int,
        val end: Int,
        val tensors: HashMap<String, FloatArray>,
        val bytes: Long,
    )

    private class Session(
        /** `kv[idx]` = cache of layer `start + idx`. */
        val kv: List<KvLayer>,
        var kvLen: Int,
    )

    companion object {
        /**
         * Concurrent forward sessions (KV-cache continuations). The cap bounds
         * per-engine RAM growth: each session multiplies a full context-length
         * cache. Same constant as the Rust engine's `MAX_SESSIONS`.
         */
        const val MAX_SESSIONS = 16

        /** The per-block tensors the qwen2 forward pass consumes. */
        private val BLOCK_TENSORS_REQUIRED = arrayOf(
            "attn_norm.weight",
            "ffn_norm.weight",
            "attn_q.weight",
            "attn_k.weight",
            "attn_v.weight",
            "attn_output.weight",
            "ffn_gate.weight",
            "ffn_up.weight",
            "ffn_down.weight",
        )

        /** Required-selected per-block extras (must exist in the optional map when the block ships them). */
        private val BLOCK_TENSORS_OPTIONAL_PREFIX = listOf(
            "attn_q_norm.weight",
            "attn_k_norm.weight",
            "attn_q.bias",
            "attn_k.bias",
            "attn_v.bias",
        )

        /**
         * `deviceName` unused here (identity comes from the RPC layer); a model
         * file that fails GGUF validation throws and the engine is refused.
         */
        fun open(modelPath: File): WorkerEngine {
            val file = try {
                GgufFile.open(modelPath)
            } catch (e: GgufException) {
                throw IllegalStateException("model: ${e.message}", e)
            }
            return WorkerEngine(file, file.modelName)
        }
    }

    private val hparams: ModelHparams
        get() = model.hparams

    @Synchronized
    fun close() {
        model.close()
        loaded = null
        sessions.clear()
    }

    /** The number of key/value elements per layer row (mirrors the Rust engine's `kv_dim`). */
    private fun kvDim(): Int = hparams.headCountKv * hparams.headDim

    @Synchronized
    override fun loadLayers(modelId: String, startLayer: Int, endLayer: Int): EngineOutcome {
        val blockCount = hparams.blockCount

        // Validation order: cheap refusals before any tensor materialization.
        if (modelId != modelName) {
            return LoadFailure(
                "unknown model_id \"$modelId\"; this worker holds \"$modelName\"",
            )
        }
        if (startLayer < 0 || startLayer > endLayer || endLayer >= blockCount) {
            return LoadFailure(
                "invalid layer range $startLayer..=$endLayer for a model with $blockCount blocks",
            )
        }
        if (loaded != null) {
            return LoadFailure("layers already loaded; unload before loading again")
        }

        // Ownership rule: token_embd iff `start == 0` (stage-0 input), OR the
        // model is tied (no `output.weight`) and this range ends the model —
        // a tied model's head is `token_embd` reused, so the split's tail
        // stage must hold it too. Plus the 9+optional blk.{i}.* tensors per
        // owned layer, and the head (output_norm, plus `output.weight` when
        // the model is untied) iff `end` is the last block.
        val names = ArrayList<String>()
        val untied = model.tensor("output.weight") != null
        if (startLayer == 0 || (endLayer == blockCount - 1 && !untied)) {
            names.add("token_embd.weight")
        }
        for (i in startLayer..endLayer) {
            for (part in BLOCK_TENSORS_REQUIRED) {
                names.add("blk.$i.$part")
            }
            for (part in BLOCK_TENSORS_OPTIONAL_PREFIX) {
                val name = "blk.$i.$part"
                if (model.tensor(name) != null) {
                    names.add(name)
                }
            }
        }
        if (endLayer == blockCount - 1) {
            names.add("output_norm.weight")
            if (untied) {
                names.add("output.weight")
            }
        }

        val tensors = HashMap<String, FloatArray>(names.size)
        var bytes = 0L
        for (name in names) {
            val desc = model.tensor(name)
                ?: return LoadFailure("model is missing tensor $name")
            val ty = GgmlType.fromId(desc.ggmlType)
                ?: return LoadFailure(
                    "tensor $name has unsupported ggml type id ${desc.ggmlType}",
                )
            val raw = try {
                model.readRaw(name)
            } catch (e: GgufException) {
                return LoadFailure("tensor $name could not be read: ${e.message}")
            }
            val f32s = try {
                dequantize(ty, raw, desc.ne.toLongArray())
            } catch (e: DequantException) {
                return LoadFailure("tensor $name: ${e.message}")
            }
            bytes += (f32s.size.toLong() * 4L)
            tensors[name] = f32s
        }

        loaded = LoadedRange(startLayer, endLayer, tensors, bytes)
        return LoadSuccess(allocatedRamBytes = bytes, startLayer, endLayer)
    }

    @Synchronized
    override fun unloadLayers(modelId: String): EngineOutcome {
        if (modelId != modelName) {
            return UnloadFailure(
                "unknown model_id \"$modelId\"; this worker holds \"$modelName\"",
            )
        }
        // Never-loaded unload reports success with zero bytes so the master's
        // rollback path never gets stuck.
        val freed = loaded?.bytes ?: 0
        loaded = null
        sessions.clear()
        return UnloadSuccess(freedRamBytes = freed)
    }

    /**
     * One forward. Mirrors the Rust `execute_forward` validation chain:
     * structure and cheap checks first, session admission after, then the
     * real math. `timingUs` comes from the caller (wall-clock is owned by
     * the RPC layer).
     */
    @Synchronized
    override fun execute(
        sessionId: String,
        startLayer: Int,
        endLayer: Int,
        activationShape: LongArray,
        activation: FloatArray,
        sequencePosition: Int,
        timingUs: Long,
        lastLogitsOnly: Boolean,
    ): EngineOutcome {
        val rng = loaded
            ?: return forwardRefuse("no layers loaded")
        if (startLayer != rng.start || endLayer != rng.end) {
            return forwardRefuse(
                "forward range $startLayer..=$endLayer does not match loaded ${rng.start}..=${rng.end}",
            )
        }

        // Stage roles: start==0 carries token ids [1, n]; later stages carry
        // hidden activations [n, hidden]. FP32-only like the Rust engine.
        val hidden = hparams.embeddingLength
        val stage0 = rng.start == 0
        val shapeOk = if (stage0) {
            activationShape.size == 2 && activationShape[0] == 1L && activationShape[1] >= 1
        } else {
            activationShape.size == 2 && activationShape[0] >= 1 && activationShape[1] == hidden.toLong()
        }
        if (!shapeOk) {
            return forwardRefuse(
                "activation shape ${activationShape.contentToString()} not valid for a stage starting at " +
                    "${rng.start} (ids expect [1, n], hidden expects [n, $hidden])",
            )
        }
        val nTokens = (if (stage0) activationShape[1] else activationShape[0]).toInt()

        if (sequencePosition < 0) {
            return forwardRefuse("negative sequence_position $sequencePosition")
        }
        if (sequencePosition + nTokens > hparams.contextLength) {
            return forwardRefuse(
                "position $sequencePosition + $nTokens tokens exceeds context_length ${hparams.contextLength}",
            )
        }

        // ---- session admission -------------------------------------------------
        if (!sessions.containsKey(sessionId) && sessions.size >= MAX_SESSIONS) {
            return forwardRefuse(
                "session cap ($MAX_SESSIONS) reached; no free slot for \"$sessionId\"",
            )
        }
        val kvDim = kvDim()
        val layersOwned = rng.end - rng.start + 1
        val kvLen = sessions[sessionId]?.kvLen ?: 0
        if (sequencePosition != kvLen) {
            return forwardRefuse(
                "sequence_position $sequencePosition != cached sequence length $kvLen (out of order)",
            )
        }

        // ---- decode/validate inputs ------------------------------------------
        var x: FloatArray
        if (stage0) {
            val emb = rng.tensors["token_embd.weight"]
                ?: return forwardRefuse(
                    "stage 0 requires token_embd.weight in the loaded range",
                )
            if (activation.size != nTokens) {
                return forwardRefuse(
                    "stage-0 payload ${activation.size} f32 values != shape [1, $nTokens]",
                )
            }
            val vocab = emb.size / hidden
            val ids = IntArray(nTokens)
            for (i in 0 until nTokens) {
                val v = activation[i]
                if (!(v.isFinite() && v >= 0.0f && v.rem(1.0f) == 0.0f)) {
                    return forwardRefuse("activation[$i] = $v is not a token id")
                }
                val id = v.toInt()
                if (id >= vocab) {
                    return forwardRefuse("token id $id at/above vocab size $vocab")
                }
                ids[i] = id
            }
            x = FloatArray(nTokens * hidden)
            embedIds(ids, emb, hidden, x)
        } else {
            if (activation.size != nTokens * hidden) {
                return forwardRefuse(
                    "hidden payload ${activation.size} f32 values != shape [$nTokens x $hidden]",
                )
            }
            // The Rust engine owns a fresh Vec per call (decoded from the
            // request bytes); the Kotlin engine may be handed the caller's
            // buffer at the RPC layer, so copy here to keep that contract.
            x = activation.copyOf()
        }

        // ---- run the owned range --------------------------------------------
        // Blocks are taken out of the tensor map one at a time and restored,
        // so concurrent sessions share one copy of the weights.
        val session = sessions.getOrPut(sessionId) {
            Session(
                kv = List(layersOwned) { KvLayer(kvDim, hparams.contextLength) },
                kvLen = 0,
            )
        }
        for (idx in 0 until layersOwned) {
            val layer = rng.start + idx
            val w = BlockWeights.take(rng.tensors, layer)
            runBlock(w, hparams, session.kv[idx], x, nTokens, kvLen)
            BlockWeights.putBack(rng.tensors, layer, w)
        }
        session.kvLen = kvLen + nTokens

        // ---- head stage (last block of the model) ---------------------------
        val isHead = rng.end == hparams.blockCount - 1
        // Prefill only needs the last row's next-token distribution. Asking for
        // it explicitly keeps the head matmul and the wire payload to one row
        // (spec 2026-10-21-last-row-logits, L2); every layer above still ran
        // for every token, so the KV cache is unaffected.
        val firstRow = if (isHead && lastLogitsOnly) nTokens - 1 else 0
        return if (isHead) {
            val norm = rng.tensors["output_norm.weight"]
                ?: return forwardRefuse("head stage requires output_norm.weight")
            for (t in firstRow until nTokens) {
                rmsNorm(x, t * hidden, hidden, norm, hparams.rmsEps)
            }
            val headPart = when {
                rng.tensors.containsKey("output.weight") -> "output.weight"
                rng.tensors.containsKey("token_embd.weight") -> "token_embd.weight" // tied model: the embedding is the head
                else -> return forwardRefuse("head stage loaded, but no output embedding/head found")
            }
            val head = rng.tensors[headPart]!!
            val nVocab = head.size / hidden
            val rows = nTokens - firstRow
            val logits = FloatArray(rows * nVocab)
            for (row in 0 until rows) {
                val t = firstRow + row
                lmHead(x, t * hidden, head, hidden, logits, row * nVocab, nVocab)
            }
            ForwardResult(
                shapeOut = longArrayOf(rows.toLong(), nVocab.toLong()),
                out = logits,
                isHead = true,
            )
        } else {
            ForwardResult(
                shapeOut = longArrayOf(nTokens.toLong(), hidden.toLong()),
                out = x.copyOf(),
                isHead = false,
            )
        }
    }

    /**
     * The real `ExecuteForwardResponse.execution_time_us` is supplied by the
     * RPC wrapper (wall-clock timing is owned by the RPC layer, mirroring
     * where the Rust engine stamps `Instant::now`); the math itself is
     * timing-free so tests stay deterministic.
     */

    private fun forwardRefuse(reason: String): ForwardFailure = ForwardFailure(reason)
}
