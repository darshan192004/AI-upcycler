package org.aiupcycler.worker.rpc

import aiupcycler.rpc.ClusterRpc.BenchmarkRequest
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
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

/**
 * Device-free contract for the worker's hardware probe (spec 2026-10-21,
 * D-C9).
 *
 * The *values* a phone measures are hardware-gated and are never asserted in
 * CI. What is asserted here is the contract:
 * - the probe produces positive, finite measurements (a degenerate or
 *   optimized-away probe would report zero);
 * - the buffer size, pass count and iteration count are the spec's bounded
 *   constants, so the probe cannot grow into an unbounded workload;
 * - the `Benchmark` RPC is served over the wire with those measurements, and
 *   it works even when no compute engine is wired - a phone that has not
 *   side-loaded a GGUF can still report its own hardware.
 */
class HardwareProbeTest {

    private lateinit var server: Server
    private lateinit var channel: ManagedChannel
    private lateinit var stub: WorkerServiceGrpc.WorkerServiceBlockingStub

    private val identity = object : WorkerIdentitySource {
        override val workerId = "probe-worker-id"
        override val deviceName = "Pixel 8"
        override val hasNpuAcceleration = false
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
    fun the_probe_uses_the_specs_bounded_constants() {
        assertEquals(8 * 1024 * 1024, HardwareProbe.BANDWIDTH_BUFFER_ELEMENTS)
        assertEquals(32 * 1024 * 1024, HardwareProbe.BANDWIDTH_BUFFER_ELEMENTS * 4)
        assertTrue(HardwareProbe.BANDWIDTH_PASSES >= 1)
        assertTrue(HardwareProbe.FLOPS_ITERATIONS > 0)
    }

    @Test
    fun the_bandwidth_probe_reports_a_positive_finite_measurement() {
        val buffer = FloatArray(1024) { 1.0f }
        val mbps = HardwareProbe.probeBandwidthMbs(buffer, HardwareProbe.BANDWIDTH_PASSES)
        assertTrue("bandwidth was $mbps", mbps.isFinite() && mbps > 0.0)
        assertTrue("the buffer was not written", buffer.all { it > 1.0f })
    }

    @Test
    fun the_flops_probe_reports_a_positive_finite_measurement() {
        val flops = HardwareProbe.probeThroughputFlops(1_000_000)
        assertTrue("flops was $flops", flops.isFinite() && flops > 0.0)
    }

    @Test
    fun a_degenerate_probe_does_not_divide_by_zero() {
        val buffer = FloatArray(16) { 1.0f }
        assertEquals(0.0, HardwareProbe.probeBandwidthMbs(buffer, 0), 0.0)
        assertEquals(0.0, HardwareProbe.probeThroughputFlops(0), 0.0)
    }

    /**
     * An exhausted heap is answered, not fatal: the 32 MiB buffer is the probe's
     * only allocation, and letting `OutOfMemoryError` escape the gRPC callback
     * would take the worker down instead of letting the master count the node
     * as failed (spec 2026-10-21, D-C6).
     */
    @Test
    fun an_out_of_memory_probe_fails_the_rpc_instead_of_the_worker() {
        val name = InProcessServerBuilder.generateName()
        val failing = WorkerRpcService(identity, metrics) {
            throw OutOfMemoryError("simulated exhaustion")
        }
        val failingServer = InProcessServerBuilder.forName(name)
            .directExecutor()
            .addService(failing)
            .build()
            .start()
        val failingChannel = InProcessChannelBuilder.forName(name).directExecutor().build()
        try {
            WorkerServiceGrpc.newBlockingStub(failingChannel)
                .benchmark(BenchmarkRequest.getDefaultInstance())
            fail("a probe that cannot allocate must not return a measurement")
        } catch (error: StatusRuntimeException) {
            assertEquals(Status.Code.RESOURCE_EXHAUSTED, error.status.code)
        } finally {
            failingChannel.shutdownNow()
            failingServer.shutdownNow()
        }
    }

    @Test
    fun the_benchmark_rpc_reports_this_workers_own_measurements() {
        val response = stub.benchmark(BenchmarkRequest.getDefaultInstance())

        assertTrue(
            "bandwidth was ${response.memoryBandwidthMbs}",
            response.memoryBandwidthMbs.isFinite() && response.memoryBandwidthMbs > 0.0,
        )
        assertTrue(
            "flops was ${response.throughputFlops}",
            response.throughputFlops.isFinite() && response.throughputFlops > 0.0,
        )
        assertTrue(
            "duration was ${response.probeDurationMs}",
            response.probeDurationMs.isFinite() && response.probeDurationMs > 0.0,
        )
    }
}
