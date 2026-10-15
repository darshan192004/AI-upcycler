package org.aiupcycler.worker

import android.content.Context
import android.util.Log
import org.aiupcycler.worker.identity.WorkerIdentity
import org.aiupcycler.worker.mdns.MdnsAdvertiser
import org.aiupcycler.worker.metrics.DeviceMetrics
import org.aiupcycler.worker.rpc.WorkerProtocol
import org.aiupcycler.worker.rpc.WorkerRpcServer

/**
 * Owns the two long-lived worker services — the gRPC server and the mDNS
 * advertisement — so the UI and any future foreground service share one entry
 * point.
 */
class WorkerRuntime(context: Context) {
    private val identity = WorkerIdentity.from(context)
    private val metrics = DeviceMetrics(context)
    private val rpcServer = WorkerRpcServer(identity, metrics)
    private val advertiser =
        MdnsAdvertiser(context, metrics, "AI Upcycler Worker ${identity.deviceName}")

    val workerId: String get() = identity.workerId
    val deviceName: String get() = identity.deviceName
    val port: Int get() = WorkerProtocol.RPC_PORT

    fun start() {
        rpcServer.start()
        advertiser.start()
        Log.i(TAG, "worker $workerId serving gRPC on 0.0.0.0:$port")
    }

    fun stop() {
        advertiser.stop()
        rpcServer.stop()
    }

    private companion object {
        const val TAG = "WorkerRuntime"
    }
}
