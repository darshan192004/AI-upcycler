package org.aiupcycler.worker.engine

import java.io.File
import java.io.RandomAccessFile
import java.nio.charset.StandardCharsets

/**
 * Minimal GGUF-v3 reader: header, key/value metadata, tensor table and
 * bounded raw reads over the file.
 *
 * Mirrors the scalar path of the shared Rust reader `model-io/src/gguf.rs`
 * (`GgufFile::open`) — same required keys, same supported `qwen2` wizardry,
 * same alignment/offset math, same truncation-at-open bounds. The tokenizer
 * KV materialization is deliberately absent: the phone engine consumes token
 * ids through the wire (`ExecuteForwardRequest`), it never tokenizes.
 *
 * Layout: magic `GGUF`, version, tensor/KV counts, the KV list, the tensor
 * info table (name, dims, type, data-section offset), then a
 * `general.alignment`-aligned (default 32) data section.
 */
class ModelHparams(
    val blockCount: Int,
    val embeddingLength: Int,
    val feedForwardLength: Int,
    val headCount: Int,
    val headCountKv: Int,
    val headDim: Int,
    val rmsEps: Float,
    val ropeTheta: Float,
    val contextLength: Int,
)

class TensorDesc(
    val ggmlType: Int,
    /** GGML dims: innermost-first (`[rows, cols]` for a 2-D matrix). */
    val ne: List<Long>,
    /** Offset inside the data section. */
    val offset: Long,
    /** Encoded byte length; `null` when the ggml type id is unknown here. */
    val nbytes: Long?,
)

/** GGUF parse/refusal, mirroring `GgufError`'s wording (the engine surfaces these as load refusals). */
class GgufException(message: String, cause: Throwable? = null) : Exception(message, cause)

class GgufFile private constructor(
    private val file: RandomAccessFile,
    private val fileLen: Long,
    private val dataOffset: Long,
    val modelName: String,
    val hparams: ModelHparams,
    private val tensors: Map<String, TensorDesc>,
) : AutoCloseable {
    fun tensor(name: String): TensorDesc? = tensors[name]

    val tensorNames: Set<String>
        get() = tensors.keys

    /** The tensor's exact encoded bytes from the data section. */
    fun readRaw(name: String): ByteArray {
        val desc = tensors[name] ?: throw GgufException("tensor $name extends past the end of the file")
        val nbytes = desc.nbytes
            ?: throw GgufException("tensor $name has an unsupported ggml type id ${desc.ggmlType}")
        // Mirror the Rust reader's checked_add chain: any overflow (or any end
        // past the file) is the same out-of-bounds refusal.
        val start = dataOffset + desc.offset
        if (desc.offset < 0 || start < dataOffset || nbytes > fileLen - start) {
            throw GgufException("tensor $name extends past the end of the file")
        }
        val out = ByteArray(nbytes.toInt())
        file.seek(start)
        file.readFully(out)
        return out
    }

    override fun close() {
        file.close()
    }

    companion object {
        /** Opens the file with the same validation order as the Rust reader. */
        fun open(path: File): GgufFile {
            val file = try {
                RandomAccessFile(path, "r")
            } catch (e: java.io.IOException) {
                throw GgufException("io error: ${e.message}", e)
            }
            try {
                val fileLen = file.length()
                val cursor = Cursor(file)
                if (!cursor.readBytes(4).contentEquals("GGUF".toByteArray(Charsets.US_ASCII))) {
                    throw GgufException("not a GGUF file (bad magic)")
                }
                val version = cursor.u32()
                if (version != 3) throw GgufException("unsupported GGUF version $version (only version 3)")
                val nTensors = cursor.u64()
                val nKv = cursor.u64()

                var modelName: String? = null
                var architecture: String? = null
                var alignment: Long? = null
                var blockCount: Int? = null
                var contextLength: Int? = null
                var embeddingLength: Int? = null
                var feedForwardLength: Int? = null
                var headCount: Int? = null
                var headCountKv: Int? = null
                var keyLength: Int? = null
                var rmsEps: Float? = null
                var ropeTheta: Float? = null

                for (i in 0 until nKv) {
                    val key = cursor.string()
                    // value_type is an int32/uint32-wide enum on disk (gguf.md spec
                    // and llama.cpp's reference reader/writer both use 4 bytes).
                    val ty = cursor.u32().toLong()
                    val value = cursor.skipValue(ty)
                    when (key) {
                        "general.name" -> modelName = expectString(ty, value, key)
                        "general.architecture" -> architecture = expectString(ty, value, key)
                        "general.alignment" -> alignment = expectU64(ty, value, key)
                        "qwen2.block_count" -> blockCount = expectU64(ty, value, key).toInt()
                        "qwen2.context_length" -> contextLength = expectU64(ty, value, key).toInt()
                        "qwen2.embedding_length" -> embeddingLength = expectU64(ty, value, key).toInt()
                        "qwen2.feed_forward_length" ->
                            feedForwardLength = expectU64(ty, value, key).toInt()
                        "qwen2.attention.head_count" -> headCount = expectU64(ty, value, key).toInt()
                        "qwen2.attention.head_count_kv" ->
                            headCountKv = expectU64(ty, value, key).toInt()
                        "qwen2.attention.key_length" -> keyLength = expectU64(ty, value, key).toInt()
                        "qwen2.attention.layer_norm_rms_epsilon" ->
                            rmsEps = expectF32(ty, value, key)
                        "qwen2.rope.freq_base" -> ropeTheta = expectF32(ty, value, key)
                    }
                }

                val arch = requireValue(architecture, "general.architecture")
                if (arch != "qwen2") throw GgufException("general.architecture is not qwen2: $arch")
                val name = requireValue(modelName, "general.name")
                val blocks = requireValue(blockCount, "qwen2.block_count")
                val ctxLen = requireValue(contextLength, "qwen2.context_length")
                val embLen = requireValue(embeddingLength, "qwen2.embedding_length")
                val ffnLen = requireValue(feedForwardLength, "qwen2.feed_forward_length")
                val heads = requireValue(headCount, "qwen2.attention.head_count")
                val kvHeads =
                    requireValue(headCountKv, "qwen2.attention.head_count_kv")
                val eps = requireValue(rmsEps, "qwen2.attention.layer_norm_rms_epsilon")
                val headDim = keyLength ?: (embLen / heads.coerceAtLeast(1))
                val rope = ropeTheta ?: 10000.0f

                val tensorMap = HashMap<String, TensorDesc>(nTensors.toInt())
                for (i in 0 until nTensors) {
                    val tName = cursor.string()
                    val nDims = cursor.u32()
                    val ne = ArrayList<Long>(nDims)
                    for (d in 0 until nDims) ne.add(cursor.u64())
                    val ggmlType = cursor.u32()
                    val offset = cursor.u64()
                    if (ne.isEmpty()) throw GgufException("malformed GGUF file: tensor with zero dimensions")
                    tensorMap[tName] = TensorDesc(ggmlType, ne, offset, tensorNbytes(ggmlType.toLong(), ne))
                }

                val align = (alignment ?: 32).coerceAtLeast(1)
                if (cursor.pos + align - 1 < 0L) throw GgufException("malformed GGUF file: unexpected end of file")
                val dataOffset = ((cursor.pos + align - 1) / align) * align

                // Every sized tensor must fit in the file, so truncation fails at
                // open, not at first read.
                for ((tName, desc) in tensorMap) {
                    val nbytes = desc.nbytes ?: continue
                    val tensorStart = dataOffset + desc.offset
                    if (tensorStart < dataOffset || nbytes > fileLen - tensorStart) {
                        throw GgufException("tensor $tName extends past the end of the file")
                    }
                }

                return GgufFile(file, fileLen, dataOffset, name, ModelHparams(
                    blockCount = blocks,
                    embeddingLength = embLen,
                    feedForwardLength = ffnLen,
                    headCount = heads,
                    headCountKv = kvHeads,
                    headDim = headDim,
                    rmsEps = eps,
                    ropeTheta = rope,
                    contextLength = ctxLen,
                ), tensorMap)
            } catch (e: GgufException) {
                file.close()
                throw e
            } catch (e: java.io.IOException) {
                file.close()
                throw GgufException("io error: ${e.message}", e)
            }
        }

        /** Encoded byte length of a tensor, or `null` for type ids this reader cannot size. */
        private fun tensorNbytes(ggmlType: Long, ne: List<Long>): Long? {
            var elems = 1L
            for (d in ne) {
                elems = try {
                    Math.multiplyExact(elems, d)
                } catch (e: ArithmeticException) {
                    return null
                }
            }
            return try {
                when (ggmlType) {
                    0L -> Math.multiplyExact(elems, 4L) // F32
                    1L -> Math.multiplyExact(elems, 2L) // F16
                    2L -> blockBytes(elems, 32, 18) // Q4_0
                    6L -> blockBytes(elems, 32, 22) // Q5_0
                    7L -> blockBytes(elems, 32, 24) // Q5_1 (refused at load; sized only)
                    8L -> blockBytes(elems, 32, 34) // Q8_0
                    12L -> blockBytes(elems, 256, 144) // Q4_K
                    14L -> blockBytes(elems, 256, 210) // Q6_K
                    else -> null
                }
            } catch (e: ArithmeticException) {
                null
            }
        }

        /** Blocks of `perBlock` elements encode to `sizeBytes` bytes each. */
        private fun blockBytes(elems: Long, perBlock: Long, sizeBytes: Long): Long? {
            if (elems % perBlock != 0L) return null // quantized dims must align with block boundaries
            return Math.multiplyExact(elems / perBlock, sizeBytes)
        }

        private fun requireValue(v: Int?, key: String): Int =
            v ?: throw GgufException("missing required key/value: $key")

        private fun requireValue(v: Float?, key: String): Float =
            v ?: throw GgufException("missing required key/value: $key")

        private fun requireValue(v: String?, key: String): String =
            v ?: throw GgufException("missing required key/value: $key")

        private fun expectString(ty: Long, v: Any?, key: String): String {
            return if (v is String && ty == 8L) v else throw GgufException("key $key has an unexpected GGUF value type")
        }

        private fun expectU64(ty: Long, v: Any?, key: String): Long {
            return if (v is Long && ty != 8L) v else throw GgufException("key $key has an unexpected GGUF value type")
        }

        private fun expectF32(ty: Long, v: Any?, key: String): Float {
            return if (v is Float && ty != 8L) v else throw GgufException("key $key has an unexpected GGUF value type")
        }
    }
}

/**
 * Sequential little-endian cursor over the file, buffering ahead from the
 * single channel so the header scan never thrashes; `pos` tracks the logical
 * position so the data-section alignment computes exactly once. Every direct
 * refill and every out-of-buffer read re-seeks explicitly, so the cursor and
 * raw reads can share one channel safely (raw reads only happen after open).
 */
private class Cursor(private val file: RandomAccessFile) {
    private val buf = ByteArray(65536)
    private var bufStart = 0L
    private var bufEnd = 0L
    var pos = 0L
        private set

    fun readBytes(n: Int): ByteArray {
        val out = ByteArray(n)
        var got = 0
        while (got < n) {
            if (pos in bufStart until bufEnd) {
                val take = minOf((n - got).toLong(), bufEnd - pos).toInt()
                buf.copyInto(out, got, (pos - bufStart).toInt(), (pos - bufStart).toInt() + take)
                pos += take
                got += take
            } else {
                refill()
            }
        }
        return out
    }

    /** Buffers ahead from `pos`; throws when the file gives out. */
    private fun refill() {
        if (pos >= file.length()) throw GgufException("malformed GGUF file: unexpected end of file")
        bufStart = pos
        var loaded = 0L
        file.seek(pos)
        while (loaded < buf.size) {
            val r = file.read(buf, loaded.toInt(), buf.size - loaded.toInt())
            if (r <= 0) break
            loaded += r
        }
        if (loaded <= 0L) throw GgufException("malformed GGUF file: unexpected end of file")
        bufEnd = bufStart + loaded
    }

    fun u32(): Int {
        val b = readBytes(4)
        return (b[0].toInt() and 0xFF) or
            ((b[1].toInt() and 0xFF) shl 8) or
            ((b[2].toInt() and 0xFF) shl 16) or
            ((b[3].toInt() and 0xFF) shl 24)
    }

    fun u64(): Long {
        val lo = u32().toLong() and 0xFFFFFFFFL
        val hi = u32().toLong() and 0xFFFFFFFFL
        return lo or (hi shl 32)
    }

    fun string(): String {
        val len = u64()
        if (len > Int.MAX_VALUE) throw GgufException("malformed GGUF file: string length")
        val bytes = readBytes(len.toInt())
        return String(bytes, StandardCharsets.UTF_8)
    }

    /** Skips a KV value of the given type, materializing scalars/strings for the caller. */
    fun skipValue(ty: Long): Any? {
        return when (ty) {
            0L, 7L -> readBytes(1)[0].toLong() and 0xFF // u8, bool
            1L -> readBytes(1)[0].toLong() // i8
            2L, 3L -> u16().toLong() // u16, i16
            4L, 5L -> u32().toLong() // u32, i32
            6L -> java.lang.Float.intBitsToFloat(u32()) // f32
            8L -> string()
            9L -> { // array: element type (u32), count (u64), then values
                val elemTy = u32().toLong()
                val count = u64()
                for (i in 0 until count) skipValue(elemTy)
                count
            }
            10L, 11L -> u64() // u64, i64
            12L -> java.lang.Double.longBitsToDouble(u64()).toFloat() // f64
            else -> throw GgufException("malformed GGUF file: unknown KV value type")
        }
    }

    private fun u16(): Int {
        val b = readBytes(2)
        return (b[0].toInt() and 0xFF) or ((b[1].toInt() and 0xFF) shl 8)
    }
}
