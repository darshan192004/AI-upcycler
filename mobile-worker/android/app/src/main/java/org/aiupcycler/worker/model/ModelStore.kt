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
    }
}
