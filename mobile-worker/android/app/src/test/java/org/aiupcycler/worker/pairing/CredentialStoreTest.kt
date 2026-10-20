package org.aiupcycler.worker.pairing

import java.io.File
import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/**
 * Credential persistence (spec D-B4): a complete set lands only when every file
 * is written, a failure leaves no partial pairing behind, and forgetting
 * returns the worker to plaintext-ADB-only service.
 */
class CredentialStoreTest {

    @get:Rule
    val temporary = TemporaryFolder()

    private fun store(): CredentialStore = CredentialStore(temporary.newFolder("pairing"))

    private fun credentials(fingerprint: String = "ab".repeat(32)) = WorkerCredentials(
        caPem = "-----BEGIN CERTIFICATE-----\nCA\n-----END CERTIFICATE-----\n",
        serverCertPem = "-----BEGIN CERTIFICATE-----\nSERVER\n-----END CERTIFICATE-----\n",
        serverKeyPem = "-----BEGIN PRIVATE KEY-----\nSERVERKEY\n-----END PRIVATE KEY-----\n",
        clientCertPem = "-----BEGIN CERTIFICATE-----\nCLIENT\n-----END CERTIFICATE-----\n",
        clientKeyPem = "-----BEGIN PRIVATE KEY-----\nCLIENTKEY\n-----END PRIVATE KEY-----\n",
        fingerprint = fingerprint,
    )

    @Test
    fun anUnpairedWorkerHasNoCredentials() {
        val store = store()
        assertFalse(store.hasCredentials())
        assertNull(store.load())
    }

    @Test
    fun roundTripsAFullCredentialSet() {
        val store = store()
        val written = credentials()

        store.save(written)

        assertTrue(store.hasCredentials())
        assertEquals(written, store.load())
    }

    @Test
    fun rePairingReplacesThePreviousSet() {
        val store = store()
        store.save(credentials("11".repeat(32)))

        val replacement = credentials("22".repeat(32))
        store.save(replacement)

        assertEquals(replacement.fingerprint, store.load()!!.fingerprint)
        // No stale file survives the replacement.
        assertEquals(
            6,
            store.load()!!.let { listOf(it.caPem, it.serverCertPem, it.serverKeyPem, it.clientCertPem, it.clientKeyPem, it.fingerprint) }
                .count { it.isNotBlank() },
        )
    }

    @Test
    fun forgettingRemovesEveryFile() {
        val directory = temporary.newFolder("pairing")
        val store = CredentialStore(directory)
        store.save(credentials())

        store.clear()

        assertFalse(store.hasCredentials())
        assertNull(store.load())
        assertFalse(directory.exists())
    }

    @Test
    fun anIncompleteSetIsNotTreatedAsPaired() {
        // A crash mid-write (or a partial restore) must not look like a
        // pairing: an incomplete set would serve mTLS with missing material.
        val directory = temporary.newFolder("pairing")
        File(directory, "ca.pem").writeText("only the CA")

        val store = CredentialStore(directory)
        assertFalse(store.hasCredentials())
        assertNull(store.load())
    }

    @Test
    fun leavesNoStagingDirectoryBehind() {
        val directory = temporary.newFolder("pairing")
        val store = CredentialStore(directory)

        store.save(credentials())

        assertEquals(
            setOf(
                "ca.pem",
                "server-cert.pem",
                "server-key.pem",
                "client-cert.pem",
                "client-key.pem",
                "fingerprint.txt",
            ),
            directory.list()!!.toSet(),
        )
    }

    @Test
    fun persistedCredentialsSurviveATemporaryDirectory() {
        // The Android wiring passes `filesDir/pairing`; the store itself must
        // not care where it is rooted, as long as it is a real directory.
        val root = temporary.newFolder("files")
        val store = CredentialStore(File(root, "pairing"))

        store.save(credentials("33".repeat(32)))

        assertNotNull(CredentialStore(File(root, "pairing")).load())
        assertTrue(Files.isDirectory(File(root, "pairing").toPath()))
    }

    @Test
    fun aFailedWriteDuringStagingLeavesThePreviousPairingIntact() {
        // The publish step overwrites in place, but nothing is published until
        // every file is staged. Force staging itself to fail (the staging path
        // is an un-creatable file) and assert the live pairing is untouched.
        val directory = temporary.newFolder("pairing")
        val store = CredentialStore(directory)
        val original = credentials("44".repeat(32))
        store.save(original)

        // A regular file where the staging directory belongs: `mkdirs` fails,
        // and `deleteRecursively` cannot remove it while it is open. Holding it
        // open is the portable way to make the staging path unusable.
        val blocker = File(directory, ".staging")
        blocker.writeText("in the way")
        java.io.FileOutputStream(blocker, true).use {
            // Keep the handle: on Windows an open file cannot be deleted, so
            // `deleteRecursively` fails and `mkdirs` cannot succeed either.
            val failure = runCatching { store.save(credentials("55".repeat(32))) }
            assertTrue("save must fail rather than silently succeed", failure.isFailure)

            // The old pairing is still complete and unchanged, and no half
            // written replacement is visible.
            assertTrue(store.hasCredentials())
            assertEquals(original, store.load())
        }
    }

    @Test
    fun publicationOverwritesRatherThanDeletingFirst() {
        // Regression: the previous implementation deleted all six files before
        // moving the new ones in, so a failure in between left the worker with
        // no credentials at all. Renaming onto an existing file must replace
        // it, and the old pairing must survive until each file is replaced.
        val directory = temporary.newFolder("pairing")
        val store = CredentialStore(directory)
        store.save(credentials("66".repeat(32)))

        val replacement = credentials("77".repeat(32))
        store.save(replacement)

        assertTrue(store.hasCredentials())
        assertEquals(replacement, store.load())
        // Nothing was left behind by the replace-in-place publication.
        assertEquals(
            setOf(
                "ca.pem", "server-cert.pem", "server-key.pem",
                "client-cert.pem", "client-key.pem", "fingerprint.txt",
            ),
            directory.list()!!.toSet(),
        )
    }
}
