package org.aiupcycler.worker.rpc

import io.grpc.Server
import io.grpc.netty.NettyServerBuilder
import io.netty.handler.ssl.SslContext
import java.util.concurrent.TimeUnit
import org.aiupcycler.worker.pairing.WorkerCredentials

/**
 * Owns the worker's gRPC server lifecycle.
 *
 * Listens on all interfaces on [WorkerProtocol.RPC_PORT] so both Wi-Fi (mDNS)
 * and USB (`adb forward`) reach the same socket.
 *
 * Two transport modes, decided by whether the worker has been paired
 * (spec D-B5):
 *
 * - **Unpaired** - plaintext, exactly as before pairing existed. This is the
 *   ADB-tunnel path `security.md` carves out, and it stays the same so nothing
 *   that already works is disturbed.
 * - **Paired** - mutual TLS on the same port: TLS 1.3 only, a client
 *   certificate signed by the enrolled CA required. A plaintext client is
 *   rejected during the handshake.
 *
 * Both modes are built through [NettyServerBuilder] - the same transport
 * implementation, configured differently - rather than through
 * `ServerBuilder.forPort`, which resolves a provider through the service
 * loader at runtime. One transport means the TLS path cannot diverge from the
 * plaintext one by accident.
 *
 * The mode is fixed at [start] and changed only by stopping and starting
 * again, which is what [WorkerRuntime] does after enrollment.
 */
class WorkerRpcServer(
    private val identity: WorkerIdentitySource,
    private val metrics: DeviceMetricsSource,
    /** Compute backend; `null` until a GGUF is side-loaded (spec D-A4). */
    private val engine: EngineSource? = null,
    private val port: Int = WorkerProtocol.RPC_PORT,
) {
    private var server: Server? = null
    private var servingTls = false

    /** Whether the running server is serving mutual TLS. */
    val isTls: Boolean get() = servingTls

    fun start(credentials: WorkerCredentials? = null) {
        if (server != null) return
        val builder = NettyServerBuilder.forPort(port)
            .addService(WorkerRpcService(identity, metrics, engine))
        if (credentials != null) {
            builder.sslContext(tlsContext(credentials))
        }
        server = builder.build().start()
        servingTls = credentials != null
    }

    fun stop() {
        val running = server ?: return
        running.shutdown()
        if (!running.awaitTermination(SHUTDOWN_SECONDS, TimeUnit.SECONDS)) {
            running.shutdownNow()
        }
        server = null
        servingTls = false
    }

    /**
     * The TLS context, or a throw. [WorkerTls.serverContext] returning `null`
     * means the stored credentials cannot be parsed; that throws rather than
     * degrading to plaintext, because a paired worker silently serving
     * plaintext on the mTLS port is the failure this whole slice exists to
     * prevent.
     */
    private fun tlsContext(credentials: WorkerCredentials): SslContext =
        WorkerTls.serverContext(credentials)
            ?: throw IllegalStateException("stored pairing credentials cannot be read")

    private companion object {
        const val SHUTDOWN_SECONDS = 5L
    }
}
