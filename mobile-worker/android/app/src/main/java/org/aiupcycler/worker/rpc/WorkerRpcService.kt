package org.aiupcycler.worker.rpc

import aiupcycler.rpc.ClusterRpc.HandshakeRequest
import aiupcycler.rpc.ClusterRpc.HandshakeResponse
import aiupcycler.rpc.ClusterRpc.HealthRequest
import aiupcycler.rpc.ClusterRpc.HealthResponse
import aiupcycler.rpc.WorkerServiceGrpc
import io.grpc.stub.StreamObserver
import org.aiupcycler.worker.identity.WorkerIdentity
import org.aiupcycler.worker.metrics.DeviceMetrics

/**
 * Server implementation of `aiupcycler.rpc.WorkerService`.
 *
 * Slice 1 answers the two metadata RPCs the master needs to *discover and place*
 * this node — `Handshake` and `GetHealthStatus`. The compute RPCs
 * (`LoadLayers`, `ExecuteForward`, `UnloadLayers`) intentionally remain
 * unimplemented and surface `UNIMPLEMENTED` until the on-device inference engine
 * (llama.cpp RPC) is wired in, so the master never schedules work this node
 * cannot honour.
 */
class WorkerRpcService(
    private val identity: WorkerIdentity,
    private val metrics: DeviceMetrics,
) : WorkerServiceGrpc.WorkerServiceImplBase() {

    override fun handshake(
        request: HandshakeRequest,
        responseObserver: StreamObserver<HandshakeResponse>,
    ) {
        val snapshot = metrics.snapshot()
        val response = HandshakeResponse.newBuilder()
            .setWorkerId(identity.workerId)
            .setDeviceName(identity.deviceName)
            .setTotalRamBytes(snapshot.totalRamBytes)
            .setFreeRamBytes(snapshot.freeRamBytes)
            .setCpuCores(Runtime.getRuntime().availableProcessors())
            .setHasNpuAcceleration(identity.hasNpuAcceleration)
            .build()
        responseObserver.onNext(response)
        responseObserver.onCompleted()
    }

    override fun getHealthStatus(
        request: HealthRequest,
        responseObserver: StreamObserver<HealthResponse>,
    ) {
        val snapshot = metrics.snapshot()
        val response = HealthResponse.newBuilder()
            .setBatteryPercentage(snapshot.batteryPercentage)
            .setCpuTempCelsius(snapshot.cpuTempCelsius)
            .setFreeRamBytes(snapshot.freeRamBytes)
            .build()
        responseObserver.onNext(response)
        responseObserver.onCompleted()
    }
}
