package org.aiupcycler.worker.pairing

/**
 * The credentials the master hands a worker at enrollment
 * (`docs/api.md` section 3.4), plus the CA verified against the QR fingerprint.
 *
 * Two single-role identities: [serverCertPem]/[serverKeyPem] are what the
 * worker serves `WorkerService` behind, [clientCertPem]/[clientKeyPem] are for
 * worker-to-master links. `main`-source code does not load the client pair yet -
 * the mTLS server lifecycle (spec D-B5) is the slice that switches to it.
 */
data class WorkerCredentials(
    val caPem: String,
    val serverCertPem: String,
    val serverKeyPem: String,
    val clientCertPem: String,
    val clientKeyPem: String,
    /**
     * Lowercase hex SHA-256 of the CA certificate DER, as returned by the
     * enroll response. By the time a [WorkerCredentials] exists this has
     * already been checked against the fingerprint the user scanned or typed
     * (spec D-B3) - it is stored so the pairing screen can show what is
     * trusted, not as an unverified claim.
     */
    val fingerprint: String,
)
