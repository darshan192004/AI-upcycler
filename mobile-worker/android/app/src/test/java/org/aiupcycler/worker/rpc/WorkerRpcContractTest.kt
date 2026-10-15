package org.aiupcycler.worker.rpc

import aiupcycler.rpc.ClusterRpc.ExecuteForwardRequest
import aiupcycler.rpc.ClusterRpc.HandshakeRequest
import aiupcycler.rpc.ClusterRpc.HealthRequest
import aiupcycler.rpc.ClusterRpc.LoadLayersRequest
import aiupcycler.rpc.ClusterRpc.UnloadLayersRequest
import aiupcycler.rpc.WorkerServiceGrpc
import io.grpc.ManagedChannel
import io.grpc.Server
import io.grpc.Status
import io.grpc.StatusRuntimeException
import io.grpc.inprocess.InProcessChannelBuilder
import io.grpc.inprocess.InProcessServerBuilder
import org.aiupcycler.worker.metrics.DeviceMetrics
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * Runs the real [WorkerRpcService] over an in-process gRPC transport and calls
 * it through the generated stub.
 *
 * This is the device-free guard on the wire contract: it proves the service
 * implements `aiupcycler.rpc.WorkerService` and that the metadata replies carry
 * the fields the master reads (`core/src/rpc/client.rs`), while the compute
 * RPCs stay explicitly `UNIMPLEMENTED` until the inference engine exists.
 */
class WorkerRpcContractTest {

    private lateinit var server: Server
    private lateinit var channel: ManagedChannel
    private lateinit var stub: WorkerServiceGrpc.WorkerServiceBlockingStub

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

    @Before
    fun setUp() {
        val name = InProcessServerBuilder.generateName()
        server = InProcessServerBuilder.forName(name)
            .directExecutor()
            .addService(WorkerRpcService(identity, metrics))
            .build()
            .start()
        channel = InProcessChannelBuilder.forName(name).directExecutor().build()
        stub = WorkerServiceGrpc.newBlockingStub(channel)
    }

    @After
    fun tearDown() {
        channel.shutdownNow()
        server.shutdownNow()
    }

    @Test
    fun serviceNameMatchesTheMasterContract() {
        assertEquals("aiupcycler.rpc.WorkerService", WorkerServiceGrpc.SERVICE_NAME)
    }

    @Test
    fun handshakeReportsIdentityAndCapacity() {
        val reply = stub.handshake(HandshakeRequest.getDefaultInstance())

        assertEquals("test-worker-id", reply.workerId)
        assertEquals("Pixel 8", reply.deviceName)
        assertEquals(8_589_934_592L, reply.totalRamBytes)
        assertEquals(4_294_967_296L, reply.freeRamBytes)
        assertTrue(reply.cpuCores > 0)
        assertTrue(reply.hasNpuAcceleration)
    }

    @Test
    fun healthStatusReportsTelemetry() {
        val reply = stub.getHealthStatus(HealthRequest.getDefaultInstance())

        assertEquals(73f, reply.batteryPercentage, 0.001f)
        assertEquals(41.5f, reply.cpuTempCelsius, 0.001f)
        assertEquals(4_294_967_296L, reply.freeRamBytes)
    }

    @Test
    fun computeRpcsReportUnimplementedUntilTheEngineLands() {
        assertEquals(
            Status.Code.UNIMPLEMENTED,
            statusOf { stub.loadLayers(LoadLayersRequest.getDefaultInstance()) },
        )
        assertEquals(
            Status.Code.UNIMPLEMENTED,
            statusOf { stub.executeForward(ExecuteForwardRequest.getDefaultInstance()) },
        )
        assertEquals(
            Status.Code.UNIMPLEMENTED,
            statusOf { stub.unloadLayers(UnloadLayersRequest.getDefaultInstance()) },
        )
    }

    private fun statusOf(block: () -> Unit): Status.Code =
        try {
            block()
            throw AssertionError("expected the RPC to fail with UNIMPLEMENTED")
        } catch (e: StatusRuntimeException) {
            e.status.code
        }
}
