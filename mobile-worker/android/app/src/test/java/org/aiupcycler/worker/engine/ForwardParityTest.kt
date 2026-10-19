package org.aiupcycler.worker.engine

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.File
import java.io.FileFilter
import kotlin.math.abs
import kotlin.math.max

/**
 * Forward numeric parity: replays the committed vectors under
 * `/engine/forward` (cut from the Rust engine by
 * `worker/examples/forward_cut.rs`, never hand-edited) against this Kotlin
 * engine, fixture bytes included (single source, spec D-A2).
 *
 * Comparison bar (spec D-A2 amendment): dequant, byte accounting and ordered
 * multiply-accumulates are bit-exact everywhere, but forward outputs cross
 * one libm boundary - `powf`/`sinf`/`cosf`/`expf` in RoPE/softmax/SiLU
 * differ 1-2 ulp between MSVC/glibc/fdlibm - so outputs are compared at
 * relative <= 1e-5 (~2 ulp) with a small absolute floor near zero. The
 * formula is identical to `worker/tests/forward_parity.rs`, which pins the
 * Rust side against the same files so a Rust-side drift fails there first.
 *
 * Coverage: stage-0 prefill + decode (KV growth), mid-stage hidden handoff,
 * head-stage logits, and a full range over the tied Q4_0 fixture (Q4_0
 * dequant feeding real matmuls + the QKV bias path + the tied head).
 */
class ForwardParityTest {

    private companion object {
        /** The spec's required coverage; extra vector files replay too. */
        val REQUIRED_CASES = listOf(
            "stage0_prefill_decode",
            "mid_stage",
            "head_stage_logits",
            "full_range_tied_q4",
        )
    }

    private class StepFile(
        val pos: Int,
        val inShape: LongArray,
        val input: FloatArray,
        val outShape: LongArray,
        val output: FloatArray,
    )

    private class CaseFile(
        val fixture: String,
        val loadStart: Int,
        val loadEnd: Int,
        val session: String,
        val steps: List<StepFile>,
    )

    private fun fixture(name: String): File {
        val url = javaClass.getResource("/engine/fixtures/$name")
            ?: error("missing fixture resource /engine/fixtures/$name")
        return File(url.toURI())
    }

    private fun vectorFiles(): List<File> {
        val url = javaClass.getResource("/engine/forward")
            ?: error("missing vector resource dir /engine/forward")
        val dir = File(url.toURI())
        val files = dir.listFiles(FileFilter { it.name.endsWith(".txt") })
            ?: error("vector dir $dir is unreadable")
        return files.sortedBy { it.name }
    }

    /** Little-endian IEEE-754 hex -> F32 (the cutter writes bytes this way). */
    private fun decodeHexF32(hex: String, fileName: String): FloatArray {
        require(hex.length % 8 == 0) {
            "$fileName: hex length ${hex.length} is not a multiple of 8"
        }
        val out = FloatArray(hex.length / 8)
        for (i in out.indices) {
            var bits = 0
            for (b in 0 until 4) {
                val hi = Character.digit(hex[i * 8 + b * 2], 16)
                val lo = Character.digit(hex[i * 8 + b * 2 + 1], 16)
                require(hi >= 0 && lo >= 0) { "$fileName: bad hex digit at float $i" }
                bits = bits or (((hi shl 4) or lo) shl (8 * b))
            }
            out[i] = java.lang.Float.intBitsToFloat(bits)
        }
        return out
    }

    private fun shapeOf(v: String, fileName: String): LongArray =
        v.split(",").map { d ->
            d.toLongOrNull() ?: error("$fileName: bad dimension $d")
        }.toLongArray()

    private fun stepFrom(kv: Map<String, String>, fileName: String): StepFile {
        fun get(key: String) = kv[key] ?: error("$fileName: step missing key $key")
        return StepFile(
            pos = get("sequence_position").toInt(),
            inShape = shapeOf(get("input_shape"), fileName),
            input = decodeHexF32(get("input_hex"), fileName),
            outShape = shapeOf(get("output_shape"), fileName),
            output = decodeHexF32(get("output_hex"), fileName),
        )
    }

    /** Parses the cutter's section format: `[case]` / `[step N]` headers,
     * `key|value` lines, `#` comments. */
    private fun parseCase(file: File): CaseFile {
        val fileName = file.name
        var fixtureName = ""
        var session = ""
        var loadStart = -1
        var loadEnd = -1
        var inCase = true
        val steps = ArrayList<StepFile>()
        var cur: MutableMap<String, String>? = null

        for (raw in file.readLines()) {
            val line = raw.trim()
            if (line.isEmpty() || line.startsWith("#")) continue
            if (line == "[case]") {
                inCase = true
                continue
            }
            if (line.startsWith("[step ")) {
                require(line.endsWith("]")) { "$fileName: bad step header $line" }
                cur?.let { steps.add(stepFrom(it, fileName)) }
                cur = HashMap()
                inCase = false
                continue
            }
            val bar = line.indexOf('|')
            require(bar > 0) { "$fileName: not a key|value line: $line" }
            val key = line.substring(0, bar)
            val value = line.substring(bar + 1)
            if (inCase) {
                when (key) {
                    "fixture" -> fixtureName = value
                    "session" -> session = value
                    "load_range" -> {
                        val parts = value.split(",")
                        require(parts.size == 2) { "$fileName: bad load_range $value" }
                        loadStart = parts[0].toInt()
                        loadEnd = parts[1].toInt()
                    }
                    else -> error("$fileName: unknown case key $key")
                }
            } else {
                val target = cur ?: error("$fileName: key $key outside a step")
                target[key] = value
            }
        }
        cur?.let { steps.add(stepFrom(it, fileName)) }

        require(fixtureName.isNotEmpty()) { "$fileName: fixture missing" }
        require(session.isNotEmpty()) { "$fileName: session missing" }
        require(steps.isNotEmpty()) { "$fileName: no steps" }
        return CaseFile(fixtureName, loadStart, loadEnd, session, steps)
    }

    /**
     * The D-A2 amendment's bar, byte-for-byte the same formula as
     * `close_enough` in `worker/tests/forward_parity.rs`: relative 1e-5
     * with an absolute floor for the near-zero region where a relative
     * comparison is meaningless.
     */
    private fun assertWithinTolerance(caseName: String, step: Int, want: FloatArray, got: FloatArray): Parity {
        assertEquals("$caseName step $step: output length", want.size, got.size)
        val p = Parity()
        for (i in want.indices) {
            val w = want[i]
            val g = got[i]
            assertTrue("$caseName step $step out[$i]: committed value not finite ($w)", w.isFinite())
            assertTrue(
                "$caseName step $step out[$i]: kotlin produced non-finite $g",
                g.isFinite(),
            )
            val diff = abs(w - g)
            val tol = max(1e-7f, 1e-5f * max(abs(w), abs(g)))
            if (diff > tol) {
                fail(
                    "$caseName step $step out[$i]: rust $w vs kotlin $g " +
                        "(delta $diff exceeds the 1e-5 relative bar $tol)",
                )
            }
            p.worstAbs = max(p.worstAbs, diff)
            p.worstBarUse = max(p.worstBarUse, diff / tol)
            if (max(abs(w), abs(g)) >= 1e-2f) {
                p.worstRel = max(p.worstRel, diff / max(abs(w), abs(g)))
            }
        }
        return p
    }

    private class Parity {
        /** Worst |want-got| over all elements. */
        var worstAbs = 0f
        /** Worst |want-got| / tolerance (1.0 = exactly at the bar). */
        var worstBarUse = 0f
        /** Worst relative deviation on elements the relative bar governs
         * (magnitude >= 1e-2; below that the absolute floor applies). */
        var worstRel = 0f

        fun merge(o: Parity) {
            worstAbs = max(worstAbs, o.worstAbs)
            worstBarUse = max(worstBarUse, o.worstBarUse)
            worstRel = max(worstRel, o.worstRel)
        }
    }

    private fun replay(case: CaseFile, caseName: String) {
        val engine = WorkerEngine.open(fixture(case.fixture))
        val modelId = case.fixture.removeSuffix(".gguf")
        val loaded = engine.loadLayers(modelId, case.loadStart, case.loadEnd)
        assertTrue("$caseName: load refused: $loaded", loaded is LoadSuccess)

        val parity = Parity()
        for ((i, step) in case.steps.withIndex()) {
            val outcome = engine.execute(
                case.session,
                case.loadStart,
                case.loadEnd,
                step.inShape,
                step.input,
                step.pos,
                0L,
            )
            assertTrue("$caseName step $i: replay refused: $outcome", outcome is ForwardResult)
            outcome as ForwardResult
            assertArrayEquals(
                "$caseName step $i: output shape drifted",
                step.outShape,
                outcome.shapeOut,
            )
            parity.merge(assertWithinTolerance(caseName, i, step.output, outcome.out))
        }
        engine.close()
        println(
            "$caseName: replay ok - worst rel (|v|>=1e-2) ${"%.2e".format(parity.worstRel)}, " +
                "worst abs delta ${"%.2e".format(parity.worstAbs)}, " +
                "max bar usage ${"%.0f".format(parity.worstBarUse * 100)}%",
        )
    }

    @Test
    fun committedVectorsReplayWithinTolerance() {
        val files = vectorFiles()
        assertTrue("no committed forward vectors found", files.isNotEmpty())
        for (name in REQUIRED_CASES) {
            assertTrue(
                "missing required vector case $name.txt - regenerate with " +
                    "`cargo run -p ai-upcycler-worker --example forward_cut`",
                files.any { it.nameWithoutExtension == name },
            )
        }
        for (file in files) {
            replay(parseCase(file), file.nameWithoutExtension)
        }
    }
}
