package org.aiupcycler.worker.engine

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * The engine-side contract behind the `LoadLayers` / `ExecuteForward` /
 * `UnloadLayers` RPCs: the full refusal matrix plus load/unload byte
 * accounting. This is the Kotlin mirror of `worker/tests/load_contract.rs` +
 * the refusal half of `worker/tests/forward.rs` (spec D-A3), and every
 * expected message below is the literal the Rust reference engine
 * (`worker/src/engine.rs`) formats for the same input - the mirror claim is
 * asserted byte-for-byte, not assumed.
 *
 * Deliberate non-coverage, recorded honestly:
 * - `missing activation_tensor` and `activation data_type ... (FP32 only)`
 *   live at the RPC wrapper, not the engine (spec D-A2b: the Kotlin engine
 *   consumes a `FloatArray`, so dtype is a wire concern of slice S5).
 * - `model is missing tensor`, `could not be read`, `stage 0 requires
 *   token_embd.weight`, and `head stage requires output_norm.weight` are
 *   defensive branches the fixture writer has no knob to reach (it cannot
 *   omit a tensor), so no committed fixture provokes them; the Rust suite
 *   cannot either for the same reason.
 */
class WorkerRpcEngineTest {

    /** Fixture table from `model-io/src/fixture.rs` (the writer's own knowledge):
     * per-block F32 element count of the 11 blk tensors =
     * attn_norm 64 + ffn_norm 64 + q_norm 16 + k_norm 16 + q 64x64 +
     * k 64x32 + v 64x32 + attn_output 64x64 + gate 64x128 + up 64x128 +
     * down 128x64 = 37024. The Qwen 0.5B-class biased fixture adds the three
     * QKV biases (64 + 32 + 32) -> 37152. token_embd = 64x256 = 16384,
     * output_norm = 64, untied output.weight = 64x256 = 16384. Q4_0 storage
     * dequantizes to the same element count, so allocated bytes are element
     * count * 4 regardless of on-disk quantization.
     */
    private companion object {
        const val UNTIED = "tiny-untied-f32"
        const val TIED_Q4 = "tiny-tied-q4-0"
        const val Q5_1_REFUSAL = "tiny-q5-1-refusal"

        /** 16384 (token_embd) + 4 * 37024 (blocks) + 64 (output_norm) + 16384 (output.weight). */
        const val FULL_UNTIED_BYTES = 723_712L
        const val PARTIAL_0_1_BYTES = 361_728L // (16384 + 2 * 37024) * 4
        const val MID_2_2_BYTES = 148_096L // 37024 * 4 - no embd, no head
        const val TIED_FULL_BYTES = 363_008L // (16384 + 2 * 37152 + 64) * 4
    }

    private fun fixture(name: String): File {
        val url = javaClass.getResource("/engine/fixtures/$name")
            ?: error("missing fixture resource /engine/fixtures/$name")
        return File(url.toURI())
    }

    private fun open(modelName: String): WorkerEngine =
        WorkerEngine.open(fixture("$modelName.gguf"))

    private fun ids(vararg v: Float): FloatArray = floatArrayOf(*v)

    private fun hidden(n: Int): FloatArray =
        FloatArray(n * 64) { (it % 7) - 3f }

    /** Asserts an [EngineOutcome] is the expected refusal and returns its message. */
    private fun refuseMessage(outcome: EngineOutcome): String {
        assertTrue("expected a failure, got $outcome", outcome is ForwardFailure || outcome is LoadFailure || outcome is UnloadFailure)
        return when (outcome) {
            is ForwardFailure -> outcome.reason
            is LoadFailure -> outcome.message
            is UnloadFailure -> outcome.message
            else -> error("unreachable")
        }
    }

    // ---- load: byte accounting (mirrors load_contract.rs) --------------------

    @Test
    fun fullRangeLoadAccountsBytesAndUnloadFreesTheSame() {
        val engine = open(UNTIED)
        val loaded = engine.loadLayers(UNTIED, 0, 3)
        assertTrue("load must succeed: $loaded", loaded is LoadSuccess)
        loaded as LoadSuccess
        assertEquals(0, loaded.startLayer)
        assertEquals(3, loaded.endLayer)
        // Table math, literal recomputation and the engine must all agree.
        val fromTable = (16_384L + 4 * 37_024L + 64L + 16_384L) * 4
        assertEquals(FULL_UNTIED_BYTES, fromTable)
        assertEquals(FULL_UNTIED_BYTES, loaded.allocatedRamBytes)

        val unloaded = engine.unloadLayers(UNTIED)
        assertTrue(unloaded is UnloadSuccess)
        assertEquals(FULL_UNTIED_BYTES, (unloaded as UnloadSuccess).freedRamBytes)
        engine.close()
    }

    @Test
    fun partialLoadsAccountExactlyTheOwnedTensors() {
        val engine = open(UNTIED)

        // start==0 owns token_embd; end!=last owns no head tensors.
        val partial = engine.loadLayers(UNTIED, 0, 1)
        assertTrue(partial is LoadSuccess)
        assertEquals(PARTIAL_0_1_BYTES, (partial as LoadSuccess).allocatedRamBytes)
        engine.unloadLayers(UNTIED)

        // A mid range owns neither embd nor head: only its one block.
        val mid = engine.loadLayers(UNTIED, 2, 2)
        assertTrue(mid is LoadSuccess)
        assertEquals(MID_2_2_BYTES, (mid as LoadSuccess).allocatedRamBytes)
        engine.close()
    }

    @Test
    fun tiedBiasedFixtureAccountsEveryMaterializedTensor() {
        val engine = open(TIED_Q4)
        val loaded = engine.loadLayers(TIED_Q4, 0, 1)
        assertTrue(loaded is LoadSuccess)
        // Tied full range: token_embd (reused as the head) + 2 biased blocks
        // + output_norm; no own output.weight exists.
        val fromTable = (16_384L + 2 * 37_152L + 64L) * 4
        assertEquals(TIED_FULL_BYTES, fromTable)
        assertEquals(TIED_FULL_BYTES, (loaded as LoadSuccess).allocatedRamBytes)

        val unloaded = engine.unloadLayers(TIED_Q4)
        assertEquals(TIED_FULL_BYTES, (unloaded as UnloadSuccess).freedRamBytes)
        engine.close()
    }

    // ---- load: refusal matrix ------------------------------------------------

    @Test
    fun loadRefusalsMirrorRustWordingAndLeaveNoResidue() {
        val engine = open(UNTIED)

        val wrongId = engine.loadLayers("other-model", 0, 3)
        assertEquals(
            "unknown model_id \"other-model\"; this worker holds \"$UNTIED\"",
            refuseMessage(wrongId),
        )

        // Same three ranges load_contract.rs refuses: past the block count,
        // inverted, and negative.
        val badRanges = listOf(0 to 4, 2 to 1, -1 to 1)
        for ((start, end) in badRanges) {
            val refused = engine.loadLayers(UNTIED, start, end)
            assertEquals(
                "range $start..=$end",
                "invalid layer range $start..=$end for a model with 4 blocks",
                refuseMessage(refused),
            )
        }

        // Refusals must retain nothing: a subsequent valid load still accounts
        // the full range exactly.
        val ok = engine.loadLayers(UNTIED, 0, 3)
        assertTrue(ok is LoadSuccess)
        assertEquals(FULL_UNTIED_BYTES, (ok as LoadSuccess).allocatedRamBytes)
        engine.close()
    }

    @Test
    fun doubleLoadRefusedWithRustWording() {
        val engine = open(UNTIED)
        assertTrue(engine.loadLayers(UNTIED, 0, 1) is LoadSuccess)
        val second = engine.loadLayers(UNTIED, 0, 1)
        assertEquals(
            "layers already loaded; unload before loading again",
            refuseMessage(second),
        )
        engine.close()
    }

    @Test
    fun unsupportedQuantTypeSevenIsRefusedByName() {
        val engine = open(Q5_1_REFUSAL)
        // Range 0..=0 must materialize token_embd first, and that tensor is
        // stored as ggml type id 7 (Q5_1) - outside the supported set.
        val refused = engine.loadLayers(Q5_1_REFUSAL, 0, 0)
        assertEquals(
            "tensor token_embd.weight has unsupported ggml type id 7",
            refuseMessage(refused),
        )
        // Nothing may be retained after the loud refusal.
        val fwd = engine.execute("s", 0, 0, longArrayOf(1, 1), ids(1f), 0, 0L)
        assertEquals("no layers loaded", refuseMessage(fwd))
        engine.close()
    }

    // ---- unload: refusal + rollback contract ---------------------------------

    @Test
    fun unloadContractMirrorsRust() {
        val engine = open(UNTIED)

        // Wrong model id: refused with the same wording as load.
        val wrongId = engine.unloadLayers("other-model")
        assertEquals(
            "unknown model_id \"other-model\"; this worker holds \"$UNTIED\"",
            refuseMessage(wrongId),
        )

        // Never-loaded unload succeeds with zero so the master's rollback
        // path never gets stuck.
        val neverLoaded = engine.unloadLayers(UNTIED)
        assertTrue(neverLoaded is UnloadSuccess)
        assertEquals(0L, (neverLoaded as UnloadSuccess).freedRamBytes)

        // Then a real load/unload round trip reports the same bytes both ways.
        val loaded = engine.loadLayers(UNTIED, 0, 3)
        assertEquals(FULL_UNTIED_BYTES, (loaded as LoadSuccess).allocatedRamBytes)
        val unloaded = engine.unloadLayers(UNTIED)
        assertEquals(FULL_UNTIED_BYTES, (unloaded as UnloadSuccess).freedRamBytes)
        engine.close()
    }

    // ---- forward: refusal matrix (mirrors forward.rs) ------------------------

    @Test
    fun forwardWithoutLoadedLayersRefused() {
        val engine = open(UNTIED)
        val fwd = engine.execute("s", 0, 3, longArrayOf(1, 2), ids(1f, 2f), 0, 0L)
        assertEquals("no layers loaded", refuseMessage(fwd))
        engine.close()
    }

    @Test
    fun forwardRangeMustMatchTheLoadedRange() {
        val engine = open(UNTIED)
        assertTrue(engine.loadLayers(UNTIED, 1, 1) is LoadSuccess)

        // The "second stage of an unloaded range" case: the request carries a
        // different range than the engine holds.
        val fwd = engine.execute("s", 0, 2, longArrayOf(2, 64), hidden(2), 0, 0L)
        assertEquals(
            "forward range 0..=2 does not match loaded 1..=1",
            refuseMessage(fwd),
        )
        engine.close()
    }

    @Test
    fun stageRoleShapeRefusalsUseTheRustWording() {
        val engine = open(UNTIED)
        assertTrue(engine.loadLayers(UNTIED, 0, 1) is LoadSuccess)

        // Stage 0 must be [1, n] ids - a hidden-shaped payload is refused.
        val stage0 = engine.execute("s", 0, 1, longArrayOf(2, 64), hidden(2), 0, 0L)
        assertEquals(
            "activation shape [2, 64] not valid for a stage starting at 0 " +
                "(ids expect [1, n], hidden expects [n, 64])",
            refuseMessage(stage0),
        )
        engine.unloadLayers(UNTIED)

        // A mid stage must be [n, 64] hidden - an ids-shaped payload is refused.
        assertTrue(engine.loadLayers(UNTIED, 2, 2) is LoadSuccess)
        val mid = engine.execute("s", 2, 2, longArrayOf(1, 2), ids(1f, 2f), 0, 0L)
        assertEquals(
            "activation shape [1, 2] not valid for a stage starting at 2 " +
                "(ids expect [1, n], hidden expects [n, 64])",
            refuseMessage(mid),
        )
        engine.close()
    }

    @Test
    fun positionGatesRefuseThenAcceptTheCorrectedOrder() {
        val engine = open(UNTIED)
        assertTrue(engine.loadLayers(UNTIED, 0, 1) is LoadSuccess)

        val neg = engine.execute("s", 0, 1, longArrayOf(1, 1), ids(1f), -1, 0L)
        assertEquals("negative sequence_position -1", refuseMessage(neg))

        // 64 + 1 tokens exceeds context_length 64 (checked before admission).
        val over = engine.execute("s", 0, 1, longArrayOf(1, 1), ids(1f), 64, 0L)
        assertEquals(
            "position 64 + 1 tokens exceeds context_length 64",
            refuseMessage(over),
        )

        // Fresh session, position 1 requested while the KV cache is empty.
        val outOfOrder = engine.execute("s", 0, 1, longArrayOf(1, 1), ids(1f), 1, 0L)
        assertEquals(
            "sequence_position 1 != cached sequence length 0 (out of order)",
            refuseMessage(outOfOrder),
        )

        assertTrue(engine.execute("s", 0, 1, longArrayOf(1, 1), ids(1f), 0, 0L) is ForwardResult)

        // Position 0 already consumed: the cache grew to 1.
        val repeat = engine.execute("s", 0, 1, longArrayOf(1, 1), ids(1f), 0, 0L)
        assertEquals(
            "sequence_position 0 != cached sequence length 1 (out of order)",
            refuseMessage(repeat),
        )

        // The corrected continuation is accepted.
        val continued = engine.execute("s", 0, 1, longArrayOf(1, 1), ids(2f), 1, 0L)
        assertTrue("position 1 must continue the sequence: $continued", continued is ForwardResult)
        engine.close()
    }

    @Test
    fun payloadValidationRefusalsUseTheRustWording() {
        val engine = open(UNTIED)
        assertTrue(engine.loadLayers(UNTIED, 0, 1) is LoadSuccess)

        // Payload length must match the declared stage-0 shape.
        val shortPayload = engine.execute("s", 0, 1, longArrayOf(1, 3), ids(1f, 2f, 3f, 4f, 5f), 0, 0L)
        assertEquals(
            "stage-0 payload 5 f32 values != shape [1, 3]",
            refuseMessage(shortPayload),
        )

        // Non-integral ids are not ids.
        val fraction = engine.execute("s", 0, 1, longArrayOf(1, 2), ids(1f, 1.5f), 0, 0L)
        assertEquals(
            "activation[1] = 1.5 is not a token id",
            refuseMessage(fraction),
        )

        // id == vocab is out of bounds (vocab is 256 in the fixture).
        val aboveVocab = engine.execute("s", 0, 1, longArrayOf(1, 2), ids(1f, 256f), 0, 0L)
        assertEquals(
            "token id 256 at/above vocab size 256",
            refuseMessage(aboveVocab),
        )
        engine.unloadLayers(UNTIED)

        // Hidden payload length must match [n x hidden].
        assertTrue(engine.loadLayers(UNTIED, 2, 2) is LoadSuccess)
        val shortHidden = engine.execute("s", 2, 2, longArrayOf(3, 64), FloatArray(5), 0, 0L)
        assertEquals(
            "hidden payload 5 f32 values != shape [3 x 64]",
            refuseMessage(shortHidden),
        )
        engine.close()
    }

    @Test
    fun sessionCapRefusesTheSeventeenthConcurrentSession() {
        val engine = open(UNTIED)
        assertTrue(engine.loadLayers(UNTIED, 0, 1) is LoadSuccess)
        for (i in 0 until 16) {
            val ok = engine.execute("cap-$i", 0, 1, longArrayOf(1, 1), ids(1f), 0, 0L)
            assertTrue("session $i must fit under MAX_SESSIONS: $ok", ok is ForwardResult)
        }
        val over = engine.execute("cap-16", 0, 1, longArrayOf(1, 1), ids(1f), 0, 0L)
        assertEquals(
            "session cap (16) reached; no free slot for \"cap-16\"",
            refuseMessage(over),
        )
        engine.close()
    }

    @Test
    fun unloadClearsSessionsAndReloadStartsFresh() {
        val engine = open(UNTIED)
        assertTrue(engine.loadLayers(UNTIED, 0, 1) is LoadSuccess)
        assertTrue(engine.execute("u", 0, 1, longArrayOf(1, 1), ids(2f), 0, 0L) is ForwardResult)

        val unloaded = engine.unloadLayers(UNTIED)
        assertTrue(unloaded is UnloadSuccess)

        val after = engine.execute("u", 0, 1, longArrayOf(1, 1), ids(2f), 0, 0L)
        assertEquals("no layers loaded", refuseMessage(after))

        assertTrue(engine.loadLayers(UNTIED, 0, 1) is LoadSuccess)
        val reloaded = engine.execute("u", 0, 1, longArrayOf(1, 1), ids(2f), 0, 0L)
        assertTrue("reload must start fresh sessions: $reloaded", reloaded is ForwardResult)
        engine.close()
    }

    // ---- forward: stage output shape contract --------------------------------

    @Test
    fun stageOutputShapesMatchTheRustEngine() {
        val engine = open(UNTIED)

        // Stage 0 over the full range is the head: [n, vocab] logits.
        assertTrue(engine.loadLayers(UNTIED, 0, 3) is LoadSuccess)
        val full = engine.execute("full", 0, 3, longArrayOf(1, 2), ids(1f, 2f), 0, 0L)
        assertTrue(full is ForwardResult)
        assertArrayEquals(longArrayOf(2, 256), (full as ForwardResult).shapeOut)
        assertEquals(2 * 256, full.out.size)
        assertTrue(full.isHead)
        engine.unloadLayers(UNTIED)

        // Stage 0 without the head is partial: [n, hidden].
        assertTrue(engine.loadLayers(UNTIED, 0, 0) is LoadSuccess)
        val partial = engine.execute("part", 0, 0, longArrayOf(1, 3), ids(1f, 3f, 2f), 0, 0L)
        assertArrayEquals(longArrayOf(3, 64), (partial as ForwardResult).shapeOut)
        assertFalse(partial.isHead)
        engine.unloadLayers(UNTIED)

        // A mid stage consumes and returns hidden: [n, hidden].
        assertTrue(engine.loadLayers(UNTIED, 2, 2) is LoadSuccess)
        val mid = engine.execute("mid", 2, 2, longArrayOf(3, 64), hidden(3), 0, 0L)
        assertArrayEquals(longArrayOf(3, 64), (mid as ForwardResult).shapeOut)
        assertFalse(mid.isHead)
        engine.unloadLayers(UNTIED)

        // The head stage alone turns hidden into logits: [n, vocab].
        assertTrue(engine.loadLayers(UNTIED, 3, 3) is LoadSuccess)
        val head = engine.execute("head", 3, 3, longArrayOf(2, 64), hidden(2), 0, 0L)
        assertArrayEquals(longArrayOf(2, 256), (head as ForwardResult).shapeOut)
        assertTrue(head.isHead)
        engine.close()
    }

    /**
     * The thread-safety half of the contract: gRPC dispatches calls on a
     * thread pool, and the shared `loaded` + `sessions` state is guarded the
     * way the Rust reference guards `Arc<Mutex<EngineState>>` - one
     * `@Synchronized` entry point per RPC (engine.rs takes one `MutexGuard`
     * the same way). The in-process contract test's `directExecutor` never
     * races, so concurrency is exercised here directly: 8 threads walking
     * 20-step position ladders (60 of the fixture's 64 context tokens) on
     * one shared engine, every output required
     * bit-identical to the serial baseline for the same ids (a torn session
     * map or cross-session KV leak surfaces as a wrong or failed result),
     * and the final unload must still report exactly what the load
     * allocated. Races are probabilistic, so this is an exercise of the
     * concurrent path rather than a proof of absence - the structural
     * guarantee is the monitor mirroring the Rust Mutex. Sessions stay
     * inside the engine's cap of 16 (1 reference + 8 workers): each worker
     * owns one session and walks the same position ladder as the serial
     * reference, so every per-position output must match it bit-for-bit.
     */
    @Test
    fun parallelForwardsOnOneEngineMatchTheSerialBaseline() {
        val engine = open(UNTIED)
        assertTrue(engine.loadLayers(UNTIED, 0, 0) is LoadSuccess)
        try {
            val tokenIds = ids(1f, 3f, 2f)

            // Serial reference: one session stepping the position ladder the
            // workers will walk (3 tokens per step, position = cached KV len).
            val iterations = 20
            val expected = ArrayList<FloatArray>(iterations)
            var refPosition = 0
            repeat(iterations) {
                val step = engine.execute("ref", 0, 0, longArrayOf(1, 3), tokenIds, refPosition, 0L)
                expected += (step as ForwardResult).out
                refPosition += 3
            }

            val threads = 8
            val failures = ConcurrentLinkedQueue<String>()
            val go = CountDownLatch(1)
            val done = CountDownLatch(threads)
            repeat(threads) { t ->
                Thread {
                    try {
                        go.await()
                        repeat(iterations) { i ->
                            val outcome =
                                engine.execute("t$t", 0, 0, longArrayOf(1, 3), tokenIds, i * 3, 0L)
                            val result = outcome as? ForwardResult
                            when {
                                result == null ->
                                    failures.add("t$t/$i refused: ${(outcome as ForwardFailure).reason}")

                                !result.shapeOut.contentEquals(longArrayOf(3, 64)) ->
                                    failures.add("t$t/$i shape: ${result.shapeOut.toList()}")

                                !result.out.contentEquals(expected[i]) ->
                                    failures.add("t$t/$i diverged from the serial baseline")
                            }
                        }
                    } finally {
                        done.countDown()
                    }
                }.start()
            }
            go.countDown()
            assertTrue("workers did not finish in time", done.await(60, TimeUnit.SECONDS))
            assertTrue("parallel dispatch failures: ${failures.take(3)}", failures.isEmpty())

            // token_embd (16384) + block 0 (37024) f32s for range 0..0.
            val unloaded = engine.unloadLayers(UNTIED) as UnloadSuccess
            assertEquals(213_632L, unloaded.freedRamBytes)
        } finally {
            engine.close()
        }
    }
}
