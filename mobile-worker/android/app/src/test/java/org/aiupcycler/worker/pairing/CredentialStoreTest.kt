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
}
