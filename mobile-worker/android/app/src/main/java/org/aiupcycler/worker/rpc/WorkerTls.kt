package org.aiupcycler.worker.rpc

import io.grpc.netty.GrpcSslContexts
import io.netty.handler.ssl.ClientAuth
import io.netty.handler.ssl.SslContext
import io.netty.handler.ssl.SslContextBuilder
import java.io.ByteArrayInputStream
import java.nio.charset.StandardCharsets
import java.security.KeyFactory
import java.security.PrivateKey
import java.security.cert.CertificateFactory
import java.security.cert.X509Certificate
import java.security.spec.PKCS8EncodedKeySpec
import java.util.Base64

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
 * ## Why the PEM is parsed here instead of handed to netty
 *
 * `GrpcSslContexts.forServer(certStream, keyStream)` looks like the obvious
 * call, and it is what this code used to do - but on Android it failed for
 * every set of credentials the master issued:
 *
 * ```
 * IllegalArgumentException: Input stream not contain valid certificates
 *   at SslContextBuilder.keyManager(:413)          // the CERTIFICATE
 * Caused by: CertificateException: Conscrypt
 *   OpenSSLX509CertificateFactory$ParsingException: No certificate found
 * ```
 *
 * The certificate was fine: `openssl` parsed it, netty's own `PemReader`
 * produced bytes byte-identical to a clean base64 decode, and Conscrypt's
 * `CertificateFactory` accepted those same bytes when they arrived through a
 * plain `ByteArrayInputStream`. What it rejected was netty's internal
 * `ByteBufInputStream` wrapper around them. Rather than depend on that
 * interaction, the material is parsed to objects here and handed over as
 * objects, which skips netty's PEM/`ByteBufInputStream` path entirely.
 *
 * This is not a behaviour change on the wire. `GrpcSslContexts.forServer(InputStream,
 * InputStream)` is defined as `configure(SslContextBuilder.forServer(...))`, and
 * `configure` is what applies the gRPC SSL provider, HTTP/2 cipher suite and
 * ALPN - so calling `configure` on a builder made from parsed objects yields the
 * same context, with one less parser in the middle.
 */
object WorkerTls {

    /** The only protocol version this server will negotiate. */
    val TLS_V1_3 = listOf("TLSv1.3")

    /**
     * An `SslContext` for serving [credentials].
     *
     * Throws - chaining the parse failure that caused it - when the material
     * cannot be read, rather than returning `null`. [WorkerRpcServer] refuses to
     * start in TLS mode on that throw instead of degrading to plaintext, which
     * is the safe outcome: a worker with unreadable credentials must not fall
     * back to serving plaintext on the mTLS port.
     *
     * The cause matters as much as the refusal. Returning `null` discarded the
     * underlying exception, so the throw that followed named no cause and a real
     * on-device parse failure could only ever surface as "stored pairing
     * credentials cannot be read" - a message that explains nothing. The message
     * now also names *which* piece failed, because the certificate, the key and
     * the trust anchor all fail in ways that read alike.
     */
    fun serverContext(
        credentials: org.aiupcycler.worker.pairing.WorkerCredentials,
    ): SslContext = try {
        val certificate = parsing("server certificate") {
            certificate(credentials.serverCertPem)
        }
        // PKCS#8 carries no algorithm hint, so take it from the certificate the
        // key belongs to rather than hard-coding EC.
        val privateKey = parsing("server key") {
            privateKey(credentials.serverKeyPem, certificate.publicKey.algorithm)
        }
        val trustAnchor = parsing("trust anchor") {
            certificate(credentials.caPem)
        }

        GrpcSslContexts.configure(SslContextBuilder.forServer(privateKey, certificate))
            .trustManager(trustAnchor)
            .clientAuth(ClientAuth.REQUIRE)
            .protocols(TLS_V1_3)
            .build()
    } catch (e: Exception) {
        throw IllegalStateException(
            "stored pairing credentials cannot be read: ${e.javaClass.name}: ${e.message}",
            e,
        )
    }

    /**
     * [CertificateFactory] accepts the PEM text directly, so there is no need to
     * strip the armour and decode the body by hand.
     */
    private fun certificate(pem: String): X509Certificate =
        CertificateFactory.getInstance("X.509")
            .generateCertificate(ByteArrayInputStream(pem.toByteArray(StandardCharsets.UTF_8)))
            as X509Certificate

    /**
     * The enrollment response carries the key as an unencrypted PKCS#8 PEM, which
     * is exactly what [PKCS8EncodedKeySpec] takes once the armour and line breaks
     * are gone.
     */
    private fun privateKey(pem: String, algorithm: String): PrivateKey {
        val body = pem
            .replace(Regex("-----[^-]+-----"), "")
            .filterNot { it.isWhitespace() }
        return KeyFactory.getInstance(algorithm)
            .generatePrivate(PKCS8EncodedKeySpec(Base64.getDecoder().decode(body)))
    }

    /**
     * Names the piece that failed while keeping the original exception reachable
     * as the cause, so the message stays diagnosable from logcat alone.
     */
    private inline fun <T> parsing(what: String, block: () -> T): T = try {
        block()
    } catch (e: Exception) {
        throw IllegalArgumentException("$what: ${e.javaClass.name}: ${e.message}", e)
    }
}
