package org.aiupcycler.worker.engine

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The committed golden dequant fixtures (copies of the Rust worker crate's
 * `worker/tests/goldens` files, guarded byte-identically by the
 * `checkEngineFixtureSync` gradle task) must dequantize to bit-exact F32
 * values on the Android engine — the records were cut from the shipped model
 * file by the `worker/examples` golden_dump tool and the Kotlin port must
 * reproduce every value's bit pattern; the format's mirror is
 * `worker/tests/dequant_golden.rs`.
 */
class QuantGoldenTest {

    private fun golden(name: String): List<String> {
        val stream = javaClass.getResourceAsStream("/engine/goldens/$name.txt")
            ?: error("missing golden resource /engine/goldens/$name.txt")
        return stream.readBytes().decodeToString()
            .split("\n")
            .map { it.trimEnd() }
            .filter { it.isNotEmpty() && !it.startsWith("#") }
    }

    private fun check(records: List<String>, ty: GgmlType, blockElems: Int) {
        var checked = 0
        for (line in records) {
            val parts = line.split("|")
            assertEquals("malformed record: $line", 4, parts.size)
            val (name, block) = parts[0] to parts[1]

            val hex = parts[2]
            assertTrue("odd-length raw hex in [$name $block]", hex.length % 2 == 0)
            val raw = ByteArray(hex.length / 2)
            for (i in raw.indices) {
                raw[i] = hex.substring(i * 2, i * 2 + 2).toInt(16).toByte()
            }

            val expected = parts[3].split(",").map { Integer.parseUnsignedInt(it, 16) }

            val out = dequantize(ty, raw, longArrayOf(blockElems.toLong()))
            assertEquals("[$name $block]: got ${out.size} f32 values, fixture holds ${expected.size}", expected.size, out.size)
            for ((i, want) in expected.withIndex()) {
                val got = out[i].toRawBits()
                assertEquals(
                    "[$name $block] element $i: got 0x%08x, want 0x%08x".format(got, want),
                    want,
                    got,
                )
            }
            checked++
        }
        assertTrue("no golden records parsed for $ty", checked > 0)
    }

    @Test
    fun q5_0GoldenRecordsDequantizeBitExactly() {
        check(golden("q5_0"), GgmlType.Q5_0, 32)
    }

    @Test
    fun q8_0GoldenRecordsDequantizeBitExactly() {
        check(golden("q8_0"), GgmlType.Q8_0, 32)
    }

    @Test
    fun q4_kGoldenRecordsDequantizeBitExactly() {
        check(golden("q4_k"), GgmlType.Q4_K, 256)
    }

    @Test
    fun q6_kGoldenRecordsDequantizeBitExactly() {
        check(golden("q6_k"), GgmlType.Q6_K, 256)
    }

    /** A single flipped raw byte must change at least one dequantized value. */
    @Test
    fun corruptingARawByteChangesTheOutput() {
        for ((ty, goldenName) in listOf(
            GgmlType.Q5_0 to "q5_0",
            GgmlType.Q8_0 to "q8_0",
            GgmlType.Q4_K to "q4_k",
            GgmlType.Q6_K to "q6_k",
        )) {
            val parts = golden(goldenName)[0].split("|")
            val hex = parts[2]
            val raw = ByteArray(hex.length / 2)
            for (i in raw.indices) raw[i] = hex.substring(i * 2, i * 2 + 2).toInt(16).toByte()
            val corrupt = raw.clone()
            corrupt[0] = (corrupt[0].toInt() xor 0x01).toByte()

            val expected = dequantize(
                ty,
                raw,
                longArrayOf(if (ty.blockElems == 256L) 256L else 32L),
            )
            val got = dequantize(
                ty,
                corrupt,
                longArrayOf(if (ty.blockElems == 256L) 256L else 32L),
            )
            assertTrue(
                "$goldenName: corrupted block produced identical values",
                got.indices.any { got[it].toRawBits() != expected[it].toRawBits() },
            )
        }
    }
}
