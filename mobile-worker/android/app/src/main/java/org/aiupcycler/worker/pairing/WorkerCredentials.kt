package org.aiupcycler.worker.pairing

/**
 * The credentials the master hands a worker at enrollment
 * (`docs/api.md` section 3.4), plus the CA verified against the QR fingerprint.
 *
 * Two single-role identities: [serverCertPem]/[serverKeyPem] are what the
 * worker serves `WorkerService` behind, [clientCertPem]/[clientKeyPem] are for
 * worker-to-master links. `main`-source code does not load the client pair yet -
 * the mTLS server lifecycle (spec D-B5) is the slice that switches to it.
 *
 * [lanToken] is in memory only: pairing hands it to the secret store, and no
 * plaintext credential file holds it (plan task 7).
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
    /** `DiscoveredNode::node_id` returned by the master for this worker. */
    val nodeId: String,
    /** LAN API bearer token, held only in memory until routed to a SecretStore. */
    val lanToken: String,
    /** The master's LAN API port. */
    val lanPort: Int,
) {
    /**
     * Compatibility overload for the pre-existing mTLS-only test fixture,
     * which lives outside task 7's write surface and only needs certificates.
     * Production callers must supply the LAN fields explicitly.
     */
    constructor(
        caPem: String,
        serverCertPem: String,
        serverKeyPem: String,
        clientCertPem: String,
        clientKeyPem: String,
        fingerprint: String,
    ) : this(
        caPem = caPem,
        serverCertPem = serverCertPem,
        serverKeyPem = serverKeyPem,
        clientCertPem = clientCertPem,
        clientKeyPem = clientKeyPem,
        fingerprint = fingerprint,
        nodeId = "",
        lanToken = "",
        lanPort = 0,
    )
}
