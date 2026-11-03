package org.aiupcycler.worker.model

import java.io.File
import java.io.IOException
import java.io.InputStream
import java.nio.file.Files

/** One GGUF the worker can serve. */
data class ModelFile(
    val file: File,
    val displayName: String,
    val sizeBytes: Long,
)

/** Outcome of asking the library to accept a pushed model. */
sealed class ReceiveStart {
    data class Accepted(val resumeFrom: Long) : ReceiveStart()

    data class Refused(val reason: String) : ReceiveStart()
}

/**
 * The worker's side-loaded model library.
 *
 * Models live in app-private storage and are imported through the system
 * document picker, so no storage permission is needed. This class is plain
 * JVM code on purpose: the naming, selection and space rules are unit-tested
 * without an emulator, and the Android layer only supplies the bytes.
 */
class ModelStore(
    private val dir: File,
    private val freeSpace: () -> Long = { dir.usableSpace },
) {
    /** The transfer currently being written, if any. */
    @Volatile
    private var receiving: Receiving? = null
    /** Every complete `.gguf` in the library, sorted case-insensitively. */
    fun list(): List<ModelFile> {
        val files = dir.listFiles { file ->
            file.isFile && file.name.endsWith(EXTENSION, ignoreCase = true)
        } ?: return emptyList()
        return files
            .sortedBy { it.name.lowercase() }
            .map { ModelFile(it, it.name, it.length()) }
    }

    /** The model the worker should open on its next start, if any. */
    fun selected(): ModelFile? {
        val name = runCatching { selectionFile().readText(Charsets.UTF_8).trim() }.getOrNull()
        // A selection is a file name, never a path: rejecting separators keeps
        // a tampered selection file from pointing outside the library.
        if (name.isNullOrEmpty() || File(name).name != name) return null
        val file = File(dir, name)
        if (!file.isFile || !file.name.endsWith(EXTENSION, ignoreCase = true)) return null
        return ModelFile(file, file.name, file.length())
    }

    fun select(file: File) {
        require(file.parentFile?.canonicalFile == dir.canonicalFile) {
            "model must live in the worker's model directory"
        }
        dir.mkdirs()
        selectionFile().writeText(file.name, Charsets.UTF_8)
    }

    fun clearSelection() {
        selectionFile().delete()
    }

    /**
     * Copies [source] into the library under a safe, unique name.
     *
     * [sizeHint] is the provider's reported size; it is used only to refuse a
     * copy that cannot fit. A null hint skips the check because some document
     * providers do not report a size.
     */
    fun import(displayName: String, sizeHint: Long?, source: InputStream): ModelFile {
        val safeName = safeName(displayName)
            ?: throw IllegalArgumentException("not a .gguf file name: $displayName")
        dir.mkdirs()
        if (sizeHint != null) {
            val available = (freeSpace() - RESERVE_BYTES).coerceAtLeast(0)
            if (sizeHint > available) {
                throw IOException(
                    "not enough free space for $sizeHint bytes " +
                        "($RESERVE_BYTES bytes headroom required)",
                )
            }
        }
        val target = uniqueTarget(safeName)
        val partial = File(dir, "${target.name}.part")
        try {
            source.use { input ->
                partial.outputStream().use { output -> input.copyTo(output, BUFFER_BYTES) }
            }
            try {
                Files.move(partial.toPath(), target.toPath())
            } catch (e: IOException) {
                throw IOException("could not move the imported model into place", e)
            }
        } catch (e: Exception) {
            partial.delete()
            throw e
        }
        return ModelFile(target, target.name, target.length())
    }

    // --- pushed models (spec 2026-11-03-model-provisioning, D-P3/D-P4) -------

    /**
     * Accepts a model pushed by the master and reports how much of it the
     * library already holds.
     *
     * The digest is the resume contract: a `.part` whose sidecar records a
     * different digest or total is discarded rather than stitched onto new
     * bytes. The sidecar exists because a partial file cannot be hashed
     * against the whole-file digest until it is complete.
     */
    fun beginReceive(fileName: String, totalBytes: Long, sha256Hex: String): ReceiveStart {
        val safe = safeName(fileName)
            ?: return ReceiveStart.Refused("not a .gguf file name: $fileName")
        if (totalBytes <= 0) {
            return ReceiveStart.Refused("total_bytes must be positive")
        }
        val digest = sha256Hex.trim().lowercase()
        if (digest.length != SHA256_HEX_CHARS || digest.any { it !in HEX_DIGITS }) {
            return ReceiveStart.Refused("sha256 must be 64 lowercase hex characters")
        }
        dir.mkdirs()
        val target = uniqueTarget(safe)
        val part = File(dir, "${target.name}.part")
        val meta = File(dir, "${target.name}.part.meta")

        var resumeFrom = 0L
        if (part.isFile) {
            val recorded = readMeta(meta)
            resumeFrom = if (recorded != null &&
                recorded.first == digest &&
                recorded.second == totalBytes
            ) {
                part.length()
            } else {
                part.delete()
                0L
            }
            if (resumeFrom > totalBytes) {
                part.delete()
                resumeFrom = 0L
            }
        }
        if (!part.isFile) meta.delete()

        val needed = (totalBytes - resumeFrom) + RESERVE_BYTES_RECEIVE
        val available = freeSpace()
        if (needed > available) {
            return ReceiveStart.Refused(
                "not enough free space: need $needed bytes, have $available",
            )
        }
        meta.writeText("sha256=$digest\ntotal=$totalBytes\n", Charsets.UTF_8)
        receiving = Receiving(part, meta, target, totalBytes, digest)
        return ReceiveStart.Accepted(resumeFrom)
    }

    /**
     * Appends one chunk, which must start exactly where the previous one
     * stopped. A reordered or duplicated chunk fails instead of corrupting the
     * file.
     */
    fun appendReceive(offset: Long, data: ByteArray): Long {
        val current = receiving
            ?: throw IllegalStateException("no model push in progress")
        val written = current.part.length()
        require(offset == written) { "chunk offset $offset does not match $written written" }
        require(written + data.size <= current.totalBytes) {
            "chunk would exceed the declared total ${current.totalBytes}"
        }
        current.part.appendBytes(data)
        return current.part.length()
    }

    /** Verifies the completed file and installs it, or deletes the partial. */
    fun finishReceive(sha256Hex: String): ModelFile {
        val current = receiving
            ?: throw IllegalStateException("no model push in progress")
        receiving = null
        try {
            if (current.part.length() != current.totalBytes) {
                throw IOException(
                    "incomplete model: ${current.part.length()} of " +
                        "${current.totalBytes} bytes",
                )
            }
            val actual = sha256Of(current.part)
            if (actual != current.sha256Hex || !actual.equals(sha256Hex.trim(), ignoreCase = true)) {
                throw IOException("sha256 mismatch: expected ${current.sha256Hex}, got $actual")
            }
            Files.move(current.part.toPath(), current.target.toPath())
            current.meta.delete()
            return ModelFile(current.target, current.target.name, current.target.length())
        } catch (e: Exception) {
            current.part.delete()
            current.meta.delete()
            throw e
        }
    }

    private data class Receiving(
        val part: File,
        val meta: File,
        val target: File,
        val totalBytes: Long,
        val sha256Hex: String,
    )

    private fun readMeta(meta: File): Pair<String, Long>? {
        val lines = runCatching { meta.readLines(Charsets.UTF_8) }.getOrNull() ?: return null
        val digest = lines.firstOrNull { it.startsWith("sha256=") }
            ?.removePrefix("sha256=") ?: return null
        val total = lines.firstOrNull { it.startsWith("total=") }
            ?.removePrefix("total=")?.toLongOrNull() ?: return null
        return digest to total
    }

    private fun sha256Of(file: File): String {
        val digest = java.security.MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(BUFFER_BYTES)
            while (true) {
                val read = input.read(buffer)
                if (read < 0) break
                digest.update(buffer, 0, read)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }

    private fun safeName(displayName: String): String? {
        val base = File(displayName).name.trim()
        if (base.isEmpty() || base == "." || base == "..") return null
        if (base.contains('/') || base.contains('\\') || base.contains('\u0000')) return null
        if (!base.endsWith(EXTENSION, ignoreCase = true)) return null
        return base
    }

    private fun uniqueTarget(name: String): File {
        val first = File(dir, name)
        if (!first.exists()) return first
        val stem = name.dropLast(EXTENSION.length)
        var i = 1
        while (true) {
            val candidate = File(dir, "$stem-$i$EXTENSION")
            if (!candidate.exists()) return candidate
            i++
        }
    }

    private fun selectionFile(): File = File(dir, SELECTION_FILE)

    private companion object {
        const val EXTENSION = ".gguf"
        const val SELECTION_FILE = ".selected"
        const val BUFFER_BYTES = 1 shl 20

        /** Keep this much free after an import so the app can still write. */
        const val RESERVE_BYTES = 64L * 1024 * 1024

        /**
         * A pushed model is larger than an import and the engine maps it, so
         * the headroom is bigger (spec D-P6).
         */
        const val RESERVE_BYTES_RECEIVE = 256L * 1024 * 1024

        const val SHA256_HEX_CHARS = 64
        const val HEX_DIGITS = "0123456789abcdef"
    }
}
