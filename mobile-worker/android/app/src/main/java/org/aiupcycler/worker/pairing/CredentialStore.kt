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
 * A failure never strands the worker: every file is written to a staging
 * directory first, so nothing touches the live set until all six are on disk,
 * and any failed write attempt clears what it staged. Publication then
 * overwrites the previous files in place. See [save] for what that does and
 * does not guarantee for a failure *during* publication.
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
     * Stores [credentials], replacing any previous pairing.
     *
     * There is no portable multi-file atomic rename, so the guarantee is
     * weaker than "all or nothing" and is stated as such: the new files are
     * moved into place one at a time, each overwriting its predecessor. A
     * failure part-way through leaves a *mix* of old and new files, and
     * [hasCredentials] would still report a complete set. What it never
     * leaves is a state where the old set was deleted first and the new one
     * failed to arrive - the previous behaviour, which could strand the worker
     * with no credentials at all.
     *
     * The files are pairwise consistent because all six come from one
     * enrollment response, and a partial overwrite is only possible if the
     * write itself fails (disk full), which the caller surfaces as an
     * [java.io.IOException] and answers by re-pairing.
     *
     * @throws java.io.IOException when the set cannot be written.
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
            // Every file is written before any of them is published, so a
            // failure while writing cannot touch the live set at all.
            for ((name, value) in values) {
                File(staging, name).writeText(value)
            }
            // Publish: overwrite in place. `renameTo` replaces an existing file
            // on every filesystem this runs on; the copy fallback exists for
            // the ones that refuse a rename across directories.
            for (name in FILES) {
                val target = File(directory, name)
                if (!File(staging, name).renameTo(target)) {
                    File(staging, name).copyTo(target, overwrite = true)
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
