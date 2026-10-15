package org.aiupcycler.worker.rpc

import io.grpc.Server
import io.grpc.ServerBuilder
import java.util.concurrent.TimeUnit

/**
 * Owns the worker's gRPC server lifecycle.
 *
 * Listens on all interfaces on [WorkerProtocol.RPC_PORT] so both Wi-Fi (mDNS)
 * and USB (`adb forward`) reach the same socket.
 */
class WorkerRpcServer(
    private val identity: WorkerIdentitySource,
    private val metrics: DeviceMetricsSource,
    private val port: Int = WorkerProtocol.RPC_PORT,
) {
    private var server: Server? = null

    fun start() {
        if (server != null) return
        server = ServerBuilder.forPort(port)
            .addService(WorkerRpcService(identity, metrics))
            .build()
            .start()
    }

    fun stop() {
        val running = server ?: return
        running.shutdown()
        if (!running.awaitTermination(SHUTDOWN_SECONDS, TimeUnit.SECONDS)) {
            running.shutdownNow()
        }
        server = null
    }

    private companion object {
        const val SHUTDOWN_SECONDS = 5L
    }
}
