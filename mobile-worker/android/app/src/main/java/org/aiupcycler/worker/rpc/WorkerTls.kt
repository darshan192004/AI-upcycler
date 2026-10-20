package org.aiupcycler.worker.rpc

import io.grpc.netty.GrpcSslContexts
import io.netty.handler.ssl.ClientAuth
import io.netty.handler.ssl.SslContext
import java.io.ByteArrayInputStream
import java.nio.charset.StandardCharsets

/**
 * Builds the worker's mutual-TLS server context from paired credentials
 * (spec D-B5).
 *
 * The rules are the whole point of pairing, so they are enforced here rather
 * than left to configuration:
 *
 * - **TLS 1.3 only.** The master's rustls client is TLS-1.3-only, so there is
 *   no 1.2 middle ground to fall back to. Pinning the protocol version means a
 *   downgrade attempt fails the handshake instead of silently negotiating a
 *   version the other end would reject anyway.
 * - **Client auth required.** The CA from enrollment is the only trust anchor,
 *   so a client without a certificate signed by it never completes the
 *   handshake. This is a *server* auth rule: `clientAuth(REQUIRE)` rejects at
 *   the TLS layer, before any gRPC service code runs.
 *
 * The private key is PKCS#8 PEM, which is what the enrollment response
 * carries and what netty's PEM reader expects.
 */
object WorkerTls {

    /** The only protocol version this server will negotiate. */
    val TLS_V1_3 = listOf("TLSv1.3")

    /**
     * An `SslContext` for serving [credentials], or `null` if the PEM material
     * cannot be parsed.
     *
     * Returns `null` rather than throwing so the caller can decide what an
     * unusable pairing means for it - [WorkerRpcServer] refuses to start in TLS
     * mode, which is the safe outcome: a worker with unreadable credentials
     * must not fall back to serving plaintext on the mTLS port.
     */
    fun serverContext(credentials: org.aiupcycler.worker.pairing.WorkerCredentials): SslContext? = try {
        val certChain = ByteArrayInputStream(credentials.serverCertPem.toByteArray(StandardCharsets.UTF_8))
        val privateKey = ByteArrayInputStream(credentials.serverKeyPem.toByteArray(StandardCharsets.UTF_8))
        val trustAnchor = ByteArrayInputStream(credentials.caPem.toByteArray(StandardCharsets.UTF_8))

        GrpcSslContexts.forServer(certChain, privateKey)
            .trustManager(trustAnchor)
            .clientAuth(ClientAuth.REQUIRE)
            .protocols(TLS_V1_3)
            .build()
    } catch (e: Exception) {
        null
    }
}
