package org.aiupcycler.worker.pairing

import java.io.File
import org.aiupcycler.worker.chat.secrets.MemorySecretStore
import org.aiupcycler.worker.chat.secrets.SecretStore
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
 *
 * The LAN bearer token is *not* a credential file: it goes to the injected
 * [SecretStore], while only the non-secret node id and port are persisted as
 * files (plan task 7).
 */
class CredentialStoreTest {

    @get:Rule
    val temporary = TemporaryFolder()

    private fun store(secrets: SecretStore = MemorySecretStore()): CredentialStore =
        CredentialStore(temporary.newFolder("pairing"), secrets)

    private fun credentials(fingerprint: String = "ab".repeat(32)) = WorkerCredentials(
        caPem = "-----BEGIN CERTIFICATE-----\nCA\n-----END CERTIFICATE-----\n",
        serverCertPem = "-----BEGIN CERTIFICATE-----\nSERVER\n-----END CERTIFICATE-----\n",
        serverKeyPem = "-----BEGIN PRIVATE KEY-----\nSERVERKEY\n-----END PRIVATE KEY-----\n",
        clientCertPem = "-----BEGIN CERTIFICATE-----\nCLIENT\n-----END CERTIFICATE-----\n",
        clientKeyPem = "-----BEGIN PRIVATE KEY-----\nCLIENTKEY\n-----END PRIVATE KEY-----\n",
        fingerprint = fingerprint,
        nodeId = NODE_ID,
        lanToken = LAN_TOKEN,
        lanPort = LAN_PORT,
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
    fun roundTripsNodeIdAndLanPortButKeepsTheTokenOutOfFiles() {
        val secrets = MemorySecretStore()
        val directory = temporary.newFolder("pairing")
        val store = CredentialStore(directory, secrets)
        val written = credentials()

        store.save(written)

        val loaded = store.load()!!
        assertEquals(NODE_ID, loaded.nodeId)
        assertEquals(LAN_PORT, loaded.lanPort)
        assertEquals(written.lanToken, loaded.lanToken)
        assertEquals(LAN_TOKEN, secrets.read(SecretStore.LAN_TOKEN_KEY))
        assertFalse(File(directory, "lan_token.txt").exists())
        directory.walkTopDown().filter { it.isFile }.forEach { file ->
            assertFalse(
                "no plaintext file may hold the LAN token: ${file.name}",
                file.readText().contains(LAN_TOKEN),
            )
        }
    }

    @Test
    fun fileSetHasTwoMoreEntriesButNoTokenFile() {
        assertTrue("node_id.txt" in CredentialStore.FILES)
        assertTrue("lan_port.txt" in CredentialStore.FILES)
        assertFalse(CredentialStore.FILES.any { it.contains("token") })
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
            8,
            store.load()!!.let {
                listOf(
                    it.caPem, it.serverCertPem, it.serverKeyPem,
                    it.clientCertPem, it.clientKeyPem, it.fingerprint,
                    it.nodeId, it.lanPort.toString(),
                )
            }.count { it.isNotBlank() },
        )
    }

    @Test
    fun forgettingRemovesEveryFileAndEverySecret() {
        val directory = temporary.newFolder("pairing")
        val secrets = MemorySecretStore()
        val store = CredentialStore(directory, secrets)
        store.save(credentials())
        SecretStore.PROVIDERS.forEach { provider ->
            secrets.write(SecretStore.keyFor(provider), "sk-test-$provider")
        }

        store.clear()

        assertFalse(store.hasCredentials())
        assertNull(store.load())
        assertFalse(directory.exists())
        assertNull(secrets.read(SecretStore.LAN_TOKEN_KEY))
        SecretStore.PROVIDERS.forEach { provider ->
            assertNull(secrets.read(SecretStore.keyFor(provider)))
        }
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
                "node_id.txt",
                "lan_port.txt",
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
        assertTrue(File(root, "pairing").isDirectory)
    }

    @Test
    fun aFailedPublicationLeavesThePreviousPairingReadable() {
        // The property that matters: publication must never remove the old set
        // before the replacement is in place. Force one target file to reject
        // both `renameTo` and the copy fallback by making it a non-empty
        // directory, which is portable across platforms.
        val directory = temporary.newFolder("pairing")
        val store = CredentialStore(directory)
        val original = credentials("44".repeat(32))
        store.save(original)

        // A non-empty directory where `ca.pem` belongs: rename onto it fails,
        // and copyTo fails too (it refuses a directory target).
        val blocked = File(directory, "ca.pem")
        assertTrue(blocked.delete())
        assertTrue(blocked.mkdirs())
        assertTrue(File(blocked, "child").writeText("x") == Unit)

        val failure = runCatching { store.save(credentials("55".repeat(32))) }
        assertTrue("save must fail rather than silently succeed", failure.isFailure)

        // Crucially, the other files were not deleted first: the old
        // pairing's material is still on disk, so a re-parse of the CA would
        // not find an empty directory. Before this fix the loop deleted all
        // files up front and this state was impossible to reach.
        assertTrue(File(directory, "client-cert.pem").isFile)
        assertTrue(File(directory, "server-key.pem").isFile)
        assertTrue(File(directory, "fingerprint.txt").isFile)
        assertTrue(File(directory, "node_id.txt").isFile)
    }

    @Test
    fun publicationOverwritesRatherThanDeletingFirst() {
        // Regression: the previous implementation deleted all files before
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
                "node_id.txt", "lan_port.txt",
            ),
            directory.list()!!.toSet(),
        )
    }

    private companion object {
        const val NODE_ID = "node-test-1"
        const val LAN_TOKEN = "token-test-lan"
        const val LAN_PORT = 8090
    }
}
