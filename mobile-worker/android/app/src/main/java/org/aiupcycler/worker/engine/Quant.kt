package org.aiupcycler.worker.engine

/**
 * Weight dequantization for the ggml types the worker supports.
 *
 * Mirrors `worker/src/quant.rs` function-for-function (sources cited there:
 * llama.cpp `ggml-common.h` block layouts and `ggml-quants.c` dequant rows
 * at d81235049). Every arithmetic expression is spelled exactly as the Rust
 * byte-level ops (same operand order, same float-reassociation-free shape,
 * logical shifts on masked bytes) so the output is bit-identical; the
 * committed golden fixtures under `engine/goldens/` (copies of the Rust
 * worker crate's golden files) pin the bit patterns so any edit that changes
 * a dequantized value fails loudly on both sides.
 */

/**
 * ggml tensor types the worker can materialize; ids are the official
 * `ggml_type` values (`uint32_t` per gguf.md): 0/1/2/6/8/12/14.
 * Q5_0 (6) is in the set because Qwen's official q4_k_m assets mix Q5_0
 * tensors into attention projections. Anything else — including Q5_1 (7) —
 * is refused loudly at load time.
 */
enum class GgmlType(val id: Int) {
    F32(0),
    F16(1),
    Q4_0(2),
    Q5_0(6),
    Q8_0(8),
    Q4_K(12),
    Q6_K(14);

    /** Official ggml_type id to supported variant; unsupported ids return `null`. */
    companion object {
        fun fromId(id: Int): GgmlType? = entries.firstOrNull { it.id == id }
    }

    /** Elements per quantized block (QK4_0/QK5_0/QK8_0 = 32, QK_K = 256); `null` for contiguous F32/F16. */
    val blockElems: Long?
        get() = when (this) {
            Q4_0, Q5_0, Q8_0 -> 32L
            Q4_K, Q6_K -> 256L
            F32, F16 -> null
        }

    /** Bytes per quantized block, from ggml-common.h static asserts. */
    val blockSize: Long?
        get() = when (this) {
            Q4_0 -> 18L
            Q5_0 -> 22L
            Q8_0 -> 34L
            Q4_K -> 144L
            Q6_K -> 210L
            F32, F16 -> null
        }
}

/** Dequantization refusal; the message mirrors the Rust wording the engine surfaces on the wire. */
class DequantException(message: String) : Exception(message)

/** Whole-tensor dequantization to F32. Contract clone of `dequantize` in `worker/src/quant.rs`. */
fun dequantize(ty: GgmlType, raw: ByteArray, ne: LongArray): FloatArray {
    if (ne.isEmpty()) throw DequantException("tensor has no dimensions")
    val row = ne[0]
    var rows = 1L
    for (i in 1 until ne.size) rows = Math.multiplyExact(rows, ne[i])
    if (row == 0L || rows == 0L) throw DequantException("zero-sized tensor dims ${ne.contentToString()}")
    val total = Math.multiplyExact(row, rows)
    if (total > Int.MAX_VALUE) {
        throw DequantException("tensor dims ${ne.contentToString()} exceed address space")
    }

    val rowBytes: Long = when (ty) {
        GgmlType.F32 -> Math.multiplyExact(row, 4L)
        GgmlType.F16 -> Math.multiplyExact(row, 2L)
        GgmlType.Q4_0 -> blockRowBytes(ty, row, 32L, 18L)
        GgmlType.Q5_0 -> blockRowBytes(ty, row, 32L, 22L)
        GgmlType.Q8_0 -> blockRowBytes(ty, row, 32L, 34L)
        GgmlType.Q4_K -> blockRowBytes(ty, row, 256L, 144L)
        GgmlType.Q6_K -> blockRowBytes(ty, row, 256L, 210L)
    }
    val expected = Math.multiplyExact(rowBytes, rows)
    if (raw.size.toLong() != expected) {
        throw DequantException(
            "byte size mismatch: file holds ${raw.size} bytes, ${ty.name} needs $expected",
        )
    }

    val out = FloatArray(total.toInt())
    val rowOut = row.toInt()
    val rowBytesInt = rowBytes.toInt()
    for (r in 0 until rows.toInt()) {
        dequantRow(ty, raw, r * rowBytesInt, rowBytesInt, out, r * rowOut, rowOut)
    }
    return out
}

private fun blockRowBytes(ty: GgmlType, row: Long, block: Long, sizeBytes: Long): Long {
    if (row % block != 0L) {
        throw DequantException(
            "row length $row is not a multiple of ${ty.name} block size $block",
        )
    }
    return Math.multiplyExact(row / block, sizeBytes)
}

/**
 * One row (or one F32/F16 span): sizes already validated by [dequantize], so
 * every slice below is exact. Explicit offsets avoid per-block copies.
 */
private fun dequantRow(
    ty: GgmlType,
    src: ByteArray,
    srcOff: Int,
    srcLen: Int,
    dst: FloatArray,
    dstOff: Int,
    dstLen: Int,
) {
    when (ty) {
        GgmlType.F32 -> {
            var d = dstOff
            var s = srcOff
            for (i in 0 until dstLen) {
                dst[d++] = floatLe(src, s)
                s += 4
            }
        }

        GgmlType.F16 -> {
            var d = dstOff
            var s = srcOff
            for (i in 0 until dstLen) {
                dst[d++] = f16ToF32(u16le(src, s))
                s += 2
            }
        }

        GgmlType.Q4_0 -> {
            // dequantize_row_q4_0 (ggml-quants.c:459), block_q4_0 = 18 B
            var y = dstOff
            var b = srcOff
            while (b < srcOff + srcLen) {
                val d = f16ToF32(u16le(src, b))
                for (j in 0 until 16) {
                    val q = src[b + 2 + j].toInt() and 0xFF
                    dst[y + j] = ((q and 0x0F).toFloat() - 8.0f) * d
                    dst[y + j + 16] = ((q shr 4).toFloat() - 8.0f) * d
                }
                y += 32
                b += 18
            }
        }

        GgmlType.Q5_0 -> {
            // dequantize_row_q5_0 (ggml-quants.c:500), block_q5_0 = 22 B:
            // f16 d, u32 qh (5th bits), u8 qs[16] (nibbles);
            // xh_0 = qh bit j as 0x10, xh_1 = qh bit (j+16) via >> (j+12).
            var y = dstOff
            var b = srcOff
            while (b < srcOff + srcLen) {
                val d = f16ToF32(u16le(src, b))
                val qh = u32le(src, b + 2)
                for (j in 0 until 16) {
                    val xh0 = ((qh ushr j) shl 4) and 0x10
                    val xh1 = u32shr(qh, j + 12) and 0x10
                    val x0 = ((src[b + 6 + j].toInt() and 0x0F) or xh0) - 16
                    val x1 = (((src[b + 6 + j].toInt() and 0xFF)) shr 4 or xh1) - 16
                    dst[y + j] = x0.toFloat() * d
                    dst[y + j + 16] = x1.toFloat() * d
                }
                y += 32
                b += 22
            }
        }

        GgmlType.Q8_0 -> {
            // dequantize_row_q8_0 (ggml-quants.c:553), block_q8_0 = 34 B
            var y = dstOff
            var b = srcOff
            while (b < srcOff + srcLen) {
                val d = f16ToF32(u16le(src, b))
                for (o in 2 until 34) {
                    dst[y + o - 2] = src[b + o].toInt().toByte().toFloat() * d
                }
                y += 32
                b += 34
            }
        }

        GgmlType.Q4_K -> {
            // dequantize_row_q4_K (ggml-quants.c:1530), block_q4_K = 144 B:
            // four 64-element groups; group j uses scale pairs (is, is+1)
            // from get_scale_min_k4: 32 lows then 32 highs.
            var y = dstOff
            var b = srcOff
            while (b < srcOff + srcLen) {
                val d = f16ToF32(u16le(src, b))
                val min = f16ToF32(u16le(src, b + 2))
                val q = b + 16
                for (gi in 0 until 4) {
                    val isP = gi * 2
                    val s0m0 = getScaleMinK4(isP, src, b + 4)
                    val s1m1 = getScaleMinK4(isP + 1, src, b + 4)
                    val d1 = d * s0m0.first.toFloat()
                    val x1 = min * s0m0.second.toFloat()
                    val d2 = d * s1m1.first.toFloat()
                    val x2 = min * s1m1.second.toFloat()
                    val g = y + gi * 64
                    for (o in 0 until 32) {
                        val qv = src[q + gi * 32 + o].toInt() and 0xFF
                        dst[g + o] = d1 * (qv and 0x0F).toFloat() - x1
                        dst[g + 32 + o] = d2 * (qv shr 4).toFloat() - x2
                    }
                }
                y += 256
                b += 144
            }
        }

        GgmlType.Q6_K -> {
            // dequantize_row_q6_K (ggml-quants.c:1940), block_q6_K = 210 B:
            // two 128-element halves; q = (low nibble | high bits << 4) - 32.
            var y = dstOff
            var b = srcOff
            while (b < srcOff + srcLen) {
                val ql = b // 128 bytes
                val qh = b + 128 // 64 bytes
                val scales = b + 192 // 16 bytes
                val d = f16ToF32(u16le(src, b + 208))
                for (n in 0 until 2) {
                    val base = y + n * 128
                    val qlH = ql + n * 64
                    val qhH = qh + n * 32
                    val scH = scales + n * 8
                    for (l in 0 until 32) {
                        val isP = l / 16
                        val h = src[qhH + l].toInt() and 0xFF
                        val q1 = i8Of((src[qlH + l].toInt() and 0x0F) or ((h and 0x03) shl 4))
                        val q2 = i8Of((src[qlH + l + 32].toInt() and 0x0F) or (((h shr 2) and 0x03) shl 4))
                        val q3 = i8Of(((src[qlH + l].toInt() and 0xFF) shr 4) or (((h shr 4) and 0x03) shl 4))
                        val q4 = i8Of(
                            ((src[qlH + l + 32].toInt() and 0xFF) shr 4) or (((h shr 6) and 0x03) shl 4),
                        )
                        val s1 = src[scH + isP].toInt().toByte().toFloat()
                        val s2 = src[scH + isP + 2].toInt().toByte().toFloat()
                        val s3 = src[scH + isP + 4].toInt().toByte().toFloat()
                        val s4 = src[scH + isP + 6].toInt().toByte().toFloat()
                        dst[base + l] = d * s1 * (q1.toFloat() - 32.0f)
                        dst[base + l + 32] = d * s2 * (q2.toFloat() - 32.0f)
                        dst[base + l + 64] = d * s3 * (q3.toFloat() - 32.0f)
                        dst[base + l + 96] = d * s4 * (q4.toFloat() - 32.0f)
                    }
                }
                y += 256
                b += 210
            }
        }
    }
}

/**
 * Verbatim port of llama.cpp `get_scale_min_k4` (ggml-quants.c:880):
 * 8 scales + 8 mins packed into 12 bytes (6 bits each). All shifts are
 * logical on masked bytes, matching the Rust `u8` semantics.
 */
internal fun getScaleMinK4(j: Int, q: ByteArray, base: Int): Pair<Int, Int> =
    if (j < 4) {
        val s = q[base + j].toInt() and 63
        val m = q[base + j + 4].toInt() and 63
        s to m
    } else {
        val d = (q[base + j + 4].toInt() and 0x0F) or (((q[base + j - 4].toInt() and 0xFF) shr 6) shl 4)
        val m = ((q[base + j + 4].toInt() and 0xFF) shr 4) or (((q[base + j].toInt() and 0xFF) shr 6) shl 4)
        d to m
    }

/** IEEE 754 half to f32 without a dependency: subnormals (exp 0), inf/NaN (exp 31), normals via exact powers of two. */
internal fun f16ToF32(bits: Int): Float {
    val sign = if (bits and 0x8000 != 0) -1.0f else 1.0f
    val exp = (bits shr 10) and 0x1F
    val mantissa = (bits and 0x03FF).toFloat()
    return when (exp) {
        0 -> sign * mantissa * Math.pow(2.0, -24.0).toFloat() // m * 2^-24 (exact power of two)
        31 -> if (mantissa == 0.0f) sign * Float.POSITIVE_INFINITY else Float.NaN
        else -> sign * (1.0f + mantissa / 1024.0f) * Math.pow(2.0, (exp - 15).toDouble()).toFloat()
    }
}

private fun u16le(src: ByteArray, off: Int): Int =
    (src[off].toInt() and 0xFF) or ((src[off + 1].toInt() and 0xFF) shl 8)

private fun u32le(src: ByteArray, off: Int): Int =
    (src[off].toInt() and 0xFF) or
        ((src[off + 1].toInt() and 0xFF) shl 8) or
        ((src[off + 2].toInt() and 0xFF) shl 16) or
        ((src[off + 3].toInt() and 0xFF) shl 24)

/** Logical right shift on the u32 bit pattern held in a (possibly negative) Int. */
private fun u32shr(x: Int, n: Int): Int = x ushr n

/** Reinterpret the low 8 bits as a signed i8 (Rust `as i8`). */
private fun i8Of(x: Int): Int = x.toByte().toInt()

internal fun floatLe(src: ByteArray, off: Int): Float = java.lang.Float.intBitsToFloat(u32le(src, off))
