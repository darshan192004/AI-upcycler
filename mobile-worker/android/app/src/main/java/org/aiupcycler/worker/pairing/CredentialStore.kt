package org.aiupcycler.worker.pairing

import java.io.File

/**
 * Persists the credentials enrollment issues, as PEM files under an
 * app-private directory (spec D-B4).
 *
 * Files rather than `SharedPreferences`: the values are multi-kilobyte PEM
 * blocks, and a directory is trivially testable on the JVM lane by pointing it
 * at a temp dir. The Android side passes `Context.filesDir`; callers on
 * Android 31+ may prefer `getNoBackupFilesDir()`, and the manifest's
 * `allowBackup="false"` (spec D-B4) already keeps private keys out of cloud
 * backup either way.
 *
 * A failure never leaves a partial pairing behind: writes go to a staging
 * directory that is moved into place only after all six files are on disk, and
 * any failed attempt clears what it staged.
 */
class CredentialStore(private val directory: File) {

    /** Whether a complete credential set is present. */
    fun hasCredentials(): Boolean = FILES.all { File(directory, it).isFile }

    /** The stored credentials, or `null` when the worker is not paired. */
    fun load(): WorkerCredentials? {
        if (!hasCredentials()) return null
        return WorkerCredentials(
            caPem = read(CA),
            serverCertPem = read(SERVER_CERT),
            serverKeyPem = read(SERVER_KEY),
            clientCertPem = read(CLIENT_CERT),
            clientKeyPem = read(CLIENT_KEY),
            fingerprint = read(FINGERPRINT).trim(),
        )
    }

    /**
     * Stores [credentials], replacing any previous pairing atomically.
     *
     * @throws java.io.IOException when the set cannot be written; the previous
     * pairing (if any) is left untouched.
     */
    fun save(credentials: WorkerCredentials) {
        val staging = File(directory, STAGING)
        staging.deleteRecursively()
        if (!staging.mkdirs() && !staging.isDirectory) {
            throw java.io.IOException("cannot create credential staging dir: $staging")
        }

        val values = mapOf(
            CA to credentials.caPem,
            SERVER_CERT to credentials.serverCertPem,
            SERVER_KEY to credentials.serverKeyPem,
            CLIENT_CERT to credentials.clientCertPem,
            CLIENT_KEY to credentials.clientKeyPem,
            FINGERPRINT to credentials.fingerprint,
        )

        try {
            for ((name, value) in values) {
                File(staging, name).writeText(value)
            }
            // Commit: drop the old set, then move the complete new one in.
            for (name in FILES) File(directory, name).delete()
            for (name in FILES) {
                val moved = File(staging, name).renameTo(File(directory, name))
                if (!moved) {
                    // Copy fallback for filesystems that refuse a rename.
                    File(staging, name).copyTo(File(directory, name), overwrite = true)
                    File(staging, name).delete()
                }
            }
        } finally {
            staging.deleteRecursively()
        }

        if (!hasCredentials()) {
            throw java.io.IOException("credential set incomplete after save: $directory")
        }
    }

    /** Forgets the pairing: the worker returns to plaintext-ADB-only service. */
    fun clear() {
        directory.deleteRecursively()
    }

    private fun read(name: String): String {
        val file = File(directory, name)
        if (!file.isFile) throw java.io.IOException("missing credential file: $file")
        return file.readText()
    }

    private companion object {
        const val STAGING = ".staging"

        const val CA = "ca.pem"
        const val SERVER_CERT = "server-cert.pem"
        const val SERVER_KEY = "server-key.pem"
        const val CLIENT_CERT = "client-cert.pem"
        const val CLIENT_KEY = "client-key.pem"
        const val FINGERPRINT = "fingerprint.txt"

        val FILES = listOf(CA, SERVER_CERT, SERVER_KEY, CLIENT_CERT, CLIENT_KEY, FINGERPRINT)
    }
}
