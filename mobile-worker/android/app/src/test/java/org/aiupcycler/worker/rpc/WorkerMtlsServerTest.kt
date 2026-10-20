package org.aiupcycler.worker.rpc

import aiupcycler.rpc.ClusterRpc.HealthRequest
import aiupcycler.rpc.WorkerServiceGrpc
import io.grpc.ManagedChannel
import io.grpc.netty.GrpcSslContexts
import io.grpc.netty.NettyChannelBuilder
import java.io.File
import java.net.ServerSocket
import java.nio.charset.StandardCharsets
import java.util.Base64
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import javax.net.ssl.TrustManagerFactory
import org.aiupcycler.worker.metrics.DeviceMetrics
import org.aiupcycler.worker.pairing.WorkerCredentials
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The mTLS serving mode against real sockets (spec D-B5).
 *
 * Everything here is a real handshake: a real [WorkerRpcServer] on a real
 * loopback port, real client certificates from the committed fixtures, and the
 * JDK's own TLS stack. That matters because the three properties under test -
 * TLS 1.3 only, client auth required, and the enrolled CA as the only trust
 * anchor - are decisions the TLS layer makes, not code this repository runs. A
 * mocked handshake would prove nothing about any of them.
 *
 * The negative cases are the ones worth having: a plaintext client, a client
 * with no certificate, and a client holding a certificate from a different CA
 * must all fail. A worker that pairs and then keeps serving plaintext is the
 * exact failure this slice exists to rule out.
 */
class WorkerMtlsServerTest {

    private val identity = object : WorkerIdentitySource {
        override val workerId = "test-worker-id"
        override val deviceName = "Pixel 8"
        override val hasNpuAcceleration = true
    }

    private val metrics = object : DeviceMetricsSource {
        override fun snapshot() = DeviceMetrics.Snapshot(
            totalRamBytes = 8_589_934_592,
            freeRamBytes = 4_294_967_296,
            batteryPercentage = 73f,
            cpuTempCelsius = 41.5f,
        )
    }

    private val serving: MutableList<WorkerRpcServer> = mutableListOf()
    private val channels: MutableList<ManagedChannel> = mutableListOf()

    @After
    fun tearDown() {
        channels.forEach { runCatching { it.shutdownNow() } }
        channels.clear()
        serving.forEach { runCatching { it.stop() } }
        serving.clear()
    }

    @Test
    fun aClientWithTheEnrolledCertificateCompletesTheHandshake() {
        val port = startTlsServer()
        val channel = tlsChannel(port, CLIENT_CERT, CLIENT_KEY, CA)
        channels += channel

        val stub = WorkerServiceGrpc.newBlockingStub(channel)
            .withDeadlineAfter(20, TimeUnit.SECONDS)

        // A real request over the mutually authenticated channel, not just a
        // successful handshake: the identity comes back from the service.
        val handshake = stub.handshake(
            aiupcycler.rpc.ClusterRpc.HandshakeRequest.newBuilder()
                .setMasterId(WorkerProtocol.MASTER_ID)
                .setProtocolVersion(WorkerProtocol.PROTOCOL_VERSION)
                .build(),
        )
        assertEquals("test-worker-id", handshake.workerId)
        assertEquals("Pixel 8", handshake.deviceName)

        val health = stub.getHealthStatus(aiupcycler.rpc.ClusterRpc.HealthRequest.getDefaultInstance())
        assertEquals(73f, health.batteryPercentage, 0.001f)
    }

    @Test
    fun aPlaintextClientIsRefused() {
        val port = startTlsServer()

        // A client that skips TLS entirely must not reach the service. gRPC
        // reports the transport failure; the assertion is only that the call
        // does not succeed and no metadata comes back.
        val plaintext = NettyChannelBuilder.forAddress("127.0.0.1", port)
            .usePlaintext()
            .build()
        channels += plaintext

        val outcome = runCatching {
            WorkerServiceGrpc.newBlockingStub(plaintext)
                .withDeadlineAfter(20, TimeUnit.SECONDS)
                .getHealthStatus(HealthRequest.getDefaultInstance())
        }
        assertTrue("plaintext must fail against the mTLS port", outcome.isFailure)
    }

    @Test
    fun aClientFromADifferentCaIsRefused() {
        // The rogue pair is signed by its own CA. Presenting it must fail: the
        // worker trusts exactly the CA the user enrolled. Trusting the real CA
        // here means the client side verifies the server fine, so any failure
        // is the server rejecting this client on its own trust anchor.
        val refusal = isRefused(
            port = startTlsServer(),
            certPem = ROGUE_CERT,
            keyPem = ROGUE_KEY,
            caPem = CA,
        )
        assertTrue("a rogue-CA client must be refused", refusal)
    }

    @Test
    fun aClientWithNoCertificateIsRefused() {
        // Client auth is REQUIRED, so "no certificate" is not "trusted by
        // default" - a client that offers nothing must be rejected.
        val refusal = isRefused(
            port = startTlsServer(),
            certPem = null,
            keyPem = null,
            caPem = CA,
        )
        assertTrue("a client with no certificate must be refused", refusal)
    }

    @Test
    fun theNegotiatedProtocolIsTls13() {
        // Asserted on the live session, not on the constant: a server that
        // negotiated TLS 1.2 would have to fail this line. The master's rustls
        // client speaks 1.3 only, so the version is a compatibility
        // requirement, not a preference.
        val session = connect(
            port = startTlsServer(),
            certPem = CLIENT_CERT,
            keyPem = CLIENT_KEY,
            caPem = CA,
        ).getOrThrow()

        assertEquals("TLSv1.3", session.protocol)
        // Belt and braces: the configured list is also pinned, so a context
        // built to offer 1.2 would fail here even if the JDK happened to
        // negotiate 1.3 with it.
        assertEquals(listOf("TLSv1.3"), WorkerTls.TLS_V1_3)
    }

    @Test
    fun anUnpairedWorkerServesPlaintextAsBefore() {
        // The ADB path must be untouched: with no credentials the server is
        // exactly what it was before this slice, and the TLS mode flag is off.
        val port = freePort()
        val server = WorkerRpcServer(identity, metrics, engine = null, port = port)
        server.start(credentials = null)
        serving += server

        assertFalse(server.isTls)

        val plaintext = NettyChannelBuilder.forAddress("127.0.0.1", port)
            .usePlaintext()
            .build()
        channels += plaintext

        val reply = WorkerServiceGrpc.newBlockingStub(plaintext)
            .withDeadlineAfter(20, TimeUnit.SECONDS)
            .getHealthStatus(HealthRequest.getDefaultInstance())
        assertEquals(73f, reply.batteryPercentage, 0.001f)
    }

    @Test
    fun tlsModeIsReportedOnlyWhileCredentialsAreUsed() {
        val port = freePort()
        val server = WorkerRpcServer(identity, metrics, engine = null, port = port)

        assertFalse("not serving before start", server.isTls)
        server.start(credentials = fixtureCredentials())
        serving += server
        assertTrue("serving mTLS when paired", server.isTls)

        server.stop()
        assertFalse("stopped servers are not serving anything", server.isTls)
    }

    @Test
    fun anUnreadablePairingRefusesToStartRatherThanServingPlaintext() {
        // The one outcome that must never happen: credentials exist (so the
        // worker is "paired") but cannot be parsed, and the server quietly
        // serves plaintext on the mTLS port. It must throw instead.
        val broken = fixtureCredentials().copy(serverKeyPem = "not a private key")
        val server = WorkerRpcServer(identity, metrics, engine = null, port = freePort())

        val outcome = runCatching { server.start(credentials = broken) }
        assertTrue("unreadable credentials must not start a plaintext server", outcome.isFailure)

        // And nothing was left running.
        server.stop()
        assertFalse(server.isTls)
    }

    @Test
    fun theFixtureServerContextBuildsAndPinsTls13() {
        // Asserted on the built context's own engine, not on a constant: this
        // is the artifact the server hands to netty, so a context configured to
        // offer 1.2 fails here.
        //
        // The socket tests above cannot catch an offered 1.2 - both ends prefer
        // 1.3, so it would still negotiate 1.3 and look correct. That is why
        // this assertion is on the enabled list rather than the negotiated one.
        val context = WorkerTls.serverContext(fixtureCredentials())
        assertNotNull("the committed fixtures must build a context", context)

        val engine = context!!.newEngine(io.netty.buffer.ByteBufAllocator.DEFAULT)
        assertEquals(
            "the server must enable only TLS 1.3",
            listOf("TLSv1.3"),
            engine.enabledProtocols.toList(),
        )
        // Client auth is deliberately not asserted here: netty applies
        // `clientAuth(REQUIRE)` on the SSLEngine when the handler initialises
        // it, not on a fresh engine. Its proof is behavioural, in the
        // anonymous-client and rogue-CA tests above.
    }

    // ---- helpers ----------------------------------------------------------

    /** Starts a real TLS-mode server on an ephemeral port; returns the port. */
    private fun startTlsServer(): Int {
        val port = freePort()
        val server = WorkerRpcServer(identity, metrics, engine = null, port = port)
        server.start(credentials = fixtureCredentials())
        serving += server
        return port
    }

    private fun tlsChannel(
        port: Int,
        certPem: String,
        keyPem: String,
        caPem: String,
    ): ManagedChannel = NettyChannelBuilder.forAddress("127.0.0.1", port)
        .sslContext(
            GrpcSslContexts.forClient()
                .trustManager(File(FIXTURE_DIR, caPem))
                .keyManager(File(FIXTURE_DIR, certPem), File(FIXTURE_DIR, keyPem))
                .build(),
        )
        .build()

    /** What a completed TLS connection negotiated. */
    private data class SessionInfo(val protocol: String)

    /**
     * Connects a TLS client with the given material and returns the negotiated
     * session.
     *
     * A raw `SSLSocket` rather than a gRPC channel, deliberately: the question
     * is what the *handshake* negotiated, and a raw socket makes a failure
     * unambiguously a TLS-layer refusal rather than a gRPC status mapping.
     *
     * The post-handshake write matters. In TLS 1.3 a client-side handshake
     * completes as soon as the client has sent its certificate; the server's
     * verdict arrives afterwards, as an alert on the first I/O. Without that
     * step a client the server rejected would look successful.
     *
     * SSL failures are rethrown as-is rather than swallowed, so the negative
     * tests can tell a real refusal from a missing fixture or a dead port.
     */
    private fun connect(
        port: Int,
        certPem: String?,
        keyPem: String?,
        caPem: String,
    ): Result<SessionInfo> = runCatching {
        val trust = TrustManagerFactory.getInstance(
            TrustManagerFactory.getDefaultAlgorithm(),
        ).apply {
            init(keyStoreWithCa(caPem))
        }
        val context = SSLContext.getInstance("TLSv1.3")
        context.init(
            if (certPem != null && keyPem != null) keyManagers(certPem, keyPem) else null,
            trust.trustManagers,
            null,
        )
        (context.socketFactory.createSocket("127.0.0.1", port) as SSLSocket).use { socket ->
            socket.soTimeout = 20_000
            socket.startHandshake()
            // Force the server's client-certificate verdict to surface.
            socket.outputStream.write(0x16)
            socket.outputStream.flush()
            socket.inputStream.read()
            SessionInfo(protocol = socket.session.protocol)
        }
    }

    /**
     * Whether the server refused a client with this material, failing loudly on
     * anything that is not a TLS-layer refusal.
     *
     * The narrow catch is the point: treating every exception as "refused"
     * would let a missing fixture or an unreachable port pass the negative
     * tests while proving nothing about client auth.
     */
    private fun isRefused(
        port: Int,
        certPem: String?,
        keyPem: String?,
        caPem: String,
    ): Boolean {
        val outcome = connect(port, certPem, keyPem, caPem)
        val failure = outcome.exceptionOrNull() ?: return false
        return when (failure) {
            // What a TLS-layer refusal looks like: the handshake, the alert we
            // provoked, or the connection the server tore down.
            is javax.net.ssl.SSLException,
            is java.io.EOFException,
            is java.net.SocketException,
            -> true
            else -> throw AssertionError(
                "expected a TLS refusal, got ${failure::class.java.name}: ${failure.message}",
                failure,
            )
        }
    }

    /** A trust store holding only [pemName] as a certificate entry. */
    private fun keyStoreWithCa(pemName: String): java.security.KeyStore =
        java.security.KeyStore.getInstance("PKCS12").apply {
            load(null)
            setCertificateEntry("ca", certificate(pemName))
        }

    private fun keyManagers(certPem: String, keyPem: String): Array<javax.net.ssl.KeyManager> {
        val key = java.security.KeyFactory.getInstance("EC").generatePrivate(
            java.security.spec.PKCS8EncodedKeySpec(
                Base64.getMimeDecoder().decode(
                    File(FIXTURE_DIR, keyPem).readText()
                        .substringAfter('\n').substringBefore("-----END"),
                ),
            ),
        )
        val store = java.security.KeyStore.getInstance("PKCS12").apply {
            load(null)
            setKeyEntry("client", key, "".toCharArray(), arrayOf(certificate(certPem)))
        }
        val manager = javax.net.ssl.KeyManagerFactory.getInstance(
            javax.net.ssl.KeyManagerFactory.getDefaultAlgorithm(),
        ).apply { init(store, "".toCharArray()) }
        return manager.keyManagers
    }

    private fun certificate(pemName: String): java.security.cert.Certificate =
        java.security.cert.CertificateFactory.getInstance("X.509").run {
            File(FIXTURE_DIR, pemName).inputStream().use { generateCertificate(it) }
        }

    private fun fixtureCredentials(): WorkerCredentials = WorkerCredentials(
        caPem = pem(CA),
        serverCertPem = pem(SERVER_CERT),
        serverKeyPem = pem(SERVER_KEY),
        clientCertPem = pem(CLIENT_CERT),
        clientKeyPem = pem(CLIENT_KEY),
        fingerprint = "00".repeat(32),
    )

    private fun pem(name: String): String =
        File(FIXTURE_DIR, name).readText(StandardCharsets.UTF_8)

    private fun freePort(): Int = ServerSocket(0, 0, java.net.InetAddress.getByName("127.0.0.1")).use {
        it.localPort
    }

    private companion object {
        val FIXTURE_DIR = File("src/test/resources/pairing")

        const val CA = "ca.pem"
        const val SERVER_CERT = "server-cert.pem"
        const val SERVER_KEY = "server-key.pem"
        const val CLIENT_CERT = "client-cert.pem"
        const val CLIENT_KEY = "client-key.pem"
        const val ROGUE_CERT = "rogue-cert.pem"
        const val ROGUE_KEY = "rogue-key.pem"
    }
}
