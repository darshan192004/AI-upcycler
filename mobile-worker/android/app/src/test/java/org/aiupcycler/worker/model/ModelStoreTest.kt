package org.aiupcycler.worker.model

import java.io.ByteArrayInputStream
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.nio.file.Files
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The model library's rules: only complete GGUFs are listed, imported names
 * cannot escape the library, and a selection is a name rather than a path.
 * These are the cases that would otherwise be discovered on a phone.
 */
class ModelStoreTest {

    private fun withDir(block: (File) -> Unit) {
        val dir = Files.createTempDirectory("model-store").toFile()
        try {
            block(dir)
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun listsOnlyGgufFilesSorted() = withDir { dir ->
        File(dir, "b.gguf").writeText("b")
        File(dir, "a.GGUF").writeText("a")
        File(dir, "notes.txt").writeText("not a model")
        File(dir, ".selected").writeText("b.gguf")

        assertEquals(listOf("a.GGUF", "b.gguf"), ModelStore(dir).list().map { it.displayName })
    }

    @Test
    fun importStripsTraversalAndCopiesBytes() = withDir { dir ->
        val bytes = ByteArray(64) { it.toByte() }

        val model = ModelStore(dir).import(
            "../../evil.gguf",
            bytes.size.toLong(),
            ByteArrayInputStream(bytes),
        )

        assertEquals("evil.gguf", model.displayName)
        assertEquals(dir.canonicalFile, model.file.parentFile!!.canonicalFile)
        assertArrayEquals(bytes, model.file.readBytes())
        assertEquals(bytes.size.toLong(), model.sizeBytes)
    }

    @Test
    fun importRefusesANonGgufName() = withDir { dir ->
        assertThrows(IllegalArgumentException::class.java) {
            ModelStore(dir).import("model.bin", 1, ByteArrayInputStream(byteArrayOf(1)))
        }
        assertTrue(ModelStore(dir).list().isEmpty())
    }

    @Test
    fun importDedupesAnExistingName() = withDir { dir ->
        val store = ModelStore(dir)
        val first = store.import("m.gguf", 1, ByteArrayInputStream(byteArrayOf(1)))
        val second = store.import("m.gguf", 1, ByteArrayInputStream(byteArrayOf(2)))

        assertEquals("m.gguf", first.displayName)
        assertEquals("m-1.gguf", second.displayName)
        assertEquals(2, store.list().size)
    }

    @Test
    fun selectionRoundTripsAndIgnoresADeletedFile() = withDir { dir ->
        val store = ModelStore(dir)
        val model = store.import("m.gguf", 1, ByteArrayInputStream(byteArrayOf(1)))

        store.select(model.file)
        assertEquals(model.file, store.selected()?.file)

        model.file.delete()
        assertNull(store.selected())
    }

    @Test
    fun selectionRejectsATraversalEntry() = withDir { dir ->
        File(dir, ".selected").writeText("../evil.gguf")
        assertNull(ModelStore(dir).selected())

        File(dir, ".selected").writeText("/tmp/evil.gguf")
        assertNull(ModelStore(dir).selected())
    }

    @Test
    fun selectRefusesAFileOutsideTheLibrary() = withDir { dir ->
        val outside = File(dir.parentFile, "outside.gguf")
        assertThrows(IllegalArgumentException::class.java) {
            ModelStore(dir).select(outside)
        }
    }

    @Test
    fun importRefusesWhenTheSizeHintCannotFit() = withDir { dir ->
        val store = ModelStore(dir) { 10L * 1024 * 1024 }

        assertThrows(IOException::class.java) {
            store.import(
                "big.gguf",
                20L * 1024 * 1024,
                ByteArrayInputStream(byteArrayOf(1)),
            )
        }
        assertTrue(store.list().isEmpty())
    }

    @Test
    fun importAllowsAZeroSizeHintBelowTheReserve() = withDir { dir ->
        val store = ModelStore(dir) { 1L }

        val model = store.import("empty.gguf", 0L, ByteArrayInputStream(ByteArray(0)))

        assertEquals("empty.gguf", model.displayName)
        assertEquals(0L, model.sizeBytes)
    }

    @Test
    fun importCleansUpAPartialCopy() = withDir { dir ->
        val broken = object : InputStream() {
            override fun read(): Int = throw IOException("boom")
        }

        assertThrows(IOException::class.java) {
            ModelStore(dir).import("m.gguf", null, broken)
        }
        assertFalse(File(dir, "m.gguf").exists())
        assertFalse(File(dir, "m.gguf.part").exists())
    }

    // --- pushed models (spec 2026-11-03-model-provisioning, D-P3/D-P4) -------

    private fun digestOf(bytes: ByteArray): String =
        java.security.MessageDigest.getInstance("SHA-256")
            .digest(bytes)
            .joinToString("") { "%02x".format(it) }

    @Test
    fun freshReceiveStartsAtZero() = withDir { dir ->
        val bytes = ByteArray(8) { it.toByte() }

        val start = ModelStore(dir).beginReceive("m.gguf", bytes.size.toLong(), digestOf(bytes))

        assertEquals(ReceiveStart.Accepted(0L), start)
    }

    @Test
    fun interruptedReceiveResumesAtThePartialLength() = withDir { dir ->
        val bytes = ByteArray(8) { it.toByte() }
        val digest = digestOf(bytes)
        val first = ModelStore(dir)
        first.beginReceive("m.gguf", bytes.size.toLong(), digest)
        first.appendReceive(0, bytes.copyOfRange(0, 3))

        // A new instance is what the worker has after a process restart.
        val resumed = ModelStore(dir).beginReceive("m.gguf", bytes.size.toLong(), digest)

        assertEquals(ReceiveStart.Accepted(3L), resumed)
    }

    @Test
    fun aPartialFromADifferentDigestIsDiscarded() = withDir { dir ->
        val oldBytes = ByteArray(8) { 1 }
        val newBytes = ByteArray(8) { 2 }
        val store = ModelStore(dir)
        store.beginReceive("m.gguf", 8, digestOf(oldBytes))
        store.appendReceive(0, oldBytes.copyOfRange(0, 5))

        val start = ModelStore(dir).beginReceive("m.gguf", 8, digestOf(newBytes))

        assertEquals(ReceiveStart.Accepted(0L), start)
        assertEquals(0L, File(dir, "m.gguf.part").length())
    }

    @Test
    fun aChunkAtTheWrongOffsetIsRefused() = withDir { dir ->
        val bytes = ByteArray(8) { it.toByte() }
        val store = ModelStore(dir)
        store.beginReceive("m.gguf", 8, digestOf(bytes))
        store.appendReceive(0, bytes.copyOfRange(0, 4))

        assertThrows(IllegalArgumentException::class.java) {
            store.appendReceive(2, bytes.copyOfRange(0, 4))
        }
        assertEquals(4L, File(dir, "m.gguf.part").length())
    }

    @Test
    fun aHashMismatchDeletesThePartial() = withDir { dir ->
        val bytes = ByteArray(8) { it.toByte() }
        val store = ModelStore(dir)
        store.beginReceive("m.gguf", 8, digestOf(bytes))
        store.appendReceive(0, bytes)

        assertThrows(IOException::class.java) {
            store.finishReceive(digestOf(ByteArray(8) { 9 }))
        }
        assertFalse(File(dir, "m.gguf.part").exists())
        assertFalse(File(dir, "m.gguf.part.meta").exists())
        assertTrue(store.list().isEmpty())
    }

    @Test
    fun aTornPartialIsNotListedAsAModel() = withDir { dir ->
        val bytes = ByteArray(8) { it.toByte() }
        val store = ModelStore(dir)
        store.beginReceive("m.gguf", 8, digestOf(bytes))
        store.appendReceive(0, bytes.copyOfRange(0, 4))

        assertTrue(store.list().isEmpty())
    }

    @Test
    fun aReceiveWithoutSpaceIsRefused() = withDir { dir ->
        val store = ModelStore(dir) { 10L * 1024 * 1024 }

        val start = store.beginReceive(
            "big.gguf",
            20L * 1024 * 1024,
            digestOf(ByteArray(1)),
        )

        assertTrue(start is ReceiveStart.Refused)
        assertTrue((start as ReceiveStart.Refused).reason.contains("free space"))
    }

    @Test
    fun aCompletedReceiveInstallsTheExactBytes() = withDir { dir ->
        val bytes = ByteArray(16) { (it * 7).toByte() }
        val store = ModelStore(dir)
        store.beginReceive("m.gguf", bytes.size.toLong(), digestOf(bytes))
        store.appendReceive(0, bytes.copyOfRange(0, 9))
        store.appendReceive(9, bytes.copyOfRange(9, bytes.size))

        val model = store.finishReceive(digestOf(bytes))

        assertEquals("m.gguf", model.displayName)
        assertArrayEquals(bytes, model.file.readBytes())
        assertFalse(File(dir, "m.gguf.part").exists())
        assertFalse(File(dir, "m.gguf.part.meta").exists())
        assertEquals(listOf("m.gguf"), store.list().map { it.displayName })
    }
}
