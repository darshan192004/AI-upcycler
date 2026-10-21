package org.aiupcycler.worker.rpc

import aiupcycler.rpc.ClusterRpc.BenchmarkRequest
import aiupcycler.rpc.ClusterRpc.BenchmarkResponse
import aiupcycler.rpc.ClusterRpc.ExecuteForwardRequest
import aiupcycler.rpc.ClusterRpc.ExecuteForwardResponse
import aiupcycler.rpc.ClusterRpc.HandshakeRequest
import aiupcycler.rpc.ClusterRpc.HandshakeResponse
import aiupcycler.rpc.ClusterRpc.HealthRequest
import aiupcycler.rpc.ClusterRpc.HealthResponse
import aiupcycler.rpc.ClusterRpc.LoadLayersRequest
import aiupcycler.rpc.ClusterRpc.LoadLayersResponse
import aiupcycler.rpc.ClusterRpc.TensorBuffer
import aiupcycler.rpc.ClusterRpc.UnloadLayersRequest
import aiupcycler.rpc.ClusterRpc.UnloadLayersResponse
import aiupcycler.rpc.WorkerServiceGrpc
import com.google.protobuf.ByteString
import io.grpc.Status
import io.grpc.stub.StreamObserver
import org.aiupcycler.worker.engine.EngineOutcome
import org.aiupcycler.worker.engine.ForwardFailure
import org.aiupcycler.worker.engine.ForwardResult
import org.aiupcycler.worker.engine.LoadFailure
import org.aiupcycler.worker.engine.LoadSuccess
import org.aiupcycler.worker.engine.UnloadFailure
import org.aiupcycler.worker.engine.UnloadSuccess
import org.aiupcycler.worker.metrics.DeviceMetrics
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Identity + capability fields the `Handshake` reply is built from. */
interface WorkerIdentitySource {
    val workerId: String
    val deviceName: String
    val hasNpuAcceleration: Boolean
}

/** Telemetry the `Handshake` and `GetHealthStatus` replies are built from. */
interface DeviceMetricsSource {
    fun snapshot(): DeviceMetrics.Snapshot
}

/**
 * The compute engine the `LoadLayers` / `ExecuteForward` / `UnloadLayers`
 * replies are produced from. Dependency inversion in the same shape as
 * [WorkerIdentitySource]/[DeviceMetricsSource]; the reference implementation
 * is `org.aiupcycler.worker.engine.WorkerEngine`, whose three method
 * signatures this mirrors exactly, so wiring it needs no adapter.
 *
 * Optional by design (spec 2026-10-19 android-compute-rpcs, D-A4): a worker
 * whose GGUF has not been side-loaded yet serves metadata only and keeps the
 * honest `UNIMPLEMENTED` surface - the master then knows not to place
 * compute here, rather than being promised work that cannot run.
 */
interface EngineSource {
    fun loadLayers(modelId: String, startLayer: Int, endLayer: Int): EngineOutcome

    fun execute(
        sessionId: String,
        startLayer: Int,
        endLayer: Int,
        activationShape: LongArray,
        activation: FloatArray,
        sequencePosition: Int,
        timingUs: Long,
        lastLogitsOnly: Boolean = false,
    ): EngineOutcome

    fun unloadLayers(modelId: String): EngineOutcome
}

/**
 * Server implementation of `aiupcycler.rpc.WorkerService`.
 *
 * The two metadata RPCs the master needs to *discover and place* this node -
 * `Handshake` and `GetHealthStatus` - are answered from [identity]/[metrics].
 * The compute RPCs delegate to [engine] when one is wired (spec D-A6 drives
 * a full split lifecycle through exactly this path in the contract test)
 * and surface `UNIMPLEMENTED` when it is absent (spec D-A4), so the master
 * never schedules work this node cannot honour.
 *
 * The FP32-only activation guard lives here (spec D-A2b): the wire dtype is
 * visible only at this layer - the engine consumes plain floats - so a
 * non-FP32 or missing activation tensor is refused with `INVALID_ARGUMENT`
 * in the Rust engine's wording and validation order.
 *
 * Depending on the small interfaces (rather than Android types) keeps this
 * class runnable on the plain JVM, which is what the in-process contract
 * test exercises.
 */
class WorkerRpcService(
    private val identity: WorkerIdentitySource,
    private val metrics: DeviceMetricsSource,
    private val engine: EngineSource? = null,
    /** Injected so a test can exercise the probe's failure mapping. */
    private val probe: () -> HardwareProbe.Result = HardwareProbe::measure,
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
            .setSupportsLastRowLogits(true)
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

    override fun loadLayers(
        request: LoadLayersRequest,
        responseObserver: StreamObserver<LoadLayersResponse>,
    ) {
        val engine = engine
            ?: return responseObserver.onError(unimplemented())
        // weights_chunk_data is deliberately ignored: weights resolve
        // worker-side today (architecture.md 3.1.3); a push protocol is a
        // future proto change (spec D-A4).
        val outcome = engine.loadLayers(request.modelId, request.startLayer, request.endLayer)
        val response = when (outcome) {
            is LoadSuccess -> LoadLayersResponse.newBuilder()
                .setSuccess(true)
                .setAllocatedRamBytes(outcome.allocatedRamBytes)
                .build()

            is LoadFailure -> LoadLayersResponse.newBuilder()
                .setSuccess(false)
                .setErrorMessage(outcome.message)
                .build()

            else -> return responseObserver.onError(
                unexpectedOutcome("loadLayers", outcome),
            )
        }
        responseObserver.onNext(response)
        responseObserver.onCompleted()
    }

    override fun executeForward(
        request: ExecuteForwardRequest,
        responseObserver: StreamObserver<ExecuteForwardResponse>,
    ) {
        val engine = engine
            ?: return responseObserver.onError(unimplemented())
        // proto3 message getters return the default instance, so absence is
        // only visible through hasActivationTensor().
        if (!request.hasActivationTensor()) {
            return responseObserver.onError(
                Status.INVALID_ARGUMENT
                    .withDescription("missing activation_tensor")
                    .asRuntimeException(),
            )
        }
        val tensor = request.activationTensor
        if (tensor.dataType != TensorBuffer.DataType.FP32) {
            return responseObserver.onError(
                Status.INVALID_ARGUMENT
                    .withDescription(
                        "activation data_type ${tensor.dataType.number} not supported (FP32 only)",
                    )
                    .asRuntimeException(),
            )
        }
        val activation = FloatArray(tensor.rawData.size() / 4).also { out ->
            ByteBuffer.wrap(tensor.rawData.toByteArray())
                .order(ByteOrder.LITTLE_ENDIAN)
                .asFloatBuffer()
                .get(out)
        }
        // Wall-clock is owned by this layer (see Engine.execute: the engine
        // consumes no clock), mirroring engine.rs's internal t0 -> elapsed.
        val t0 = System.nanoTime()
        val outcome = engine.execute(
            sessionId = request.sessionId,
            startLayer = request.startLayer,
            endLayer = request.endLayer,
            activationShape = tensor.shapeList.toLongArray(),
            activation = activation,
            sequencePosition = request.sequencePosition,
            timingUs = 0L,
            lastLogitsOnly = request.lastLogitsOnly,
        )
        val elapsedUs = ((System.nanoTime() - t0) / 1_000).coerceAtLeast(1L)
        val response = when (outcome) {
            is ForwardResult -> ExecuteForwardResponse.newBuilder()
                .setSuccess(true)
                .setOutputActivationTensor(
                    TensorBuffer.newBuilder()
                        .addAllShape(outcome.shapeOut.asList())
                        .setDataType(TensorBuffer.DataType.FP32)
                        .setRawData(ByteString.copyFrom(outcome.out.toLeBytes()))
                        .build(),
                )
                .setExecutionTimeUs(elapsedUs)
                .build()

            is ForwardFailure ->
                // Refusals carry no reason on the wire (success=false, no
                // tensor, zero time) - exactly the Rust engine's shape.
                ExecuteForwardResponse.newBuilder().build()

            else -> return responseObserver.onError(
                unexpectedOutcome("executeForward", outcome),
            )
        }
        responseObserver.onNext(response)
        responseObserver.onCompleted()
    }

    override fun unloadLayers(
        request: UnloadLayersRequest,
        responseObserver: StreamObserver<UnloadLayersResponse>,
    ) {
        val engine = engine
            ?: return responseObserver.onError(unimplemented())
        val outcome = engine.unloadLayers(request.modelId)
        val response = when (outcome) {
            is UnloadSuccess -> UnloadLayersResponse.newBuilder()
                .setSuccess(true)
                .setFreedRamBytes(outcome.freedRamBytes)
                .build()

            is UnloadFailure -> UnloadLayersResponse.newBuilder()
                .setSuccess(false)
                .setErrorMessage(outcome.message)
                .build()

            else -> return responseObserver.onError(
                unexpectedOutcome("unloadLayers", outcome),
            )
        }
        responseObserver.onNext(response)
        responseObserver.onCompleted()
    }

    override fun benchmark(
        request: BenchmarkRequest,
        responseObserver: StreamObserver<BenchmarkResponse>,
    ) {
        // The probe is pure, bounded and synchronous, so it is safe to run
        // on the gRPC handler thread (spec 2026-10-21, D-C9). It measures
        // this worker's own hardware - never a value derived from a node id.
        //
        // The 32 MiB measurement buffer is the only allocation. If the device
        // cannot spare it, failing the RPC is honest: an OutOfMemoryError
        // escaping the callback would take the worker down, and the master
        // could not count the node as failed (spec D-C6).
        val measured = try {
            probe()
        } catch (error: OutOfMemoryError) {
            responseObserver.onError(
                Status.RESOURCE_EXHAUSTED
                    .withDescription("hardware probe could not allocate its measurement buffer")
                    .withCause(error)
                    .asRuntimeException(),
            )
            return
        }
        val response = BenchmarkResponse.newBuilder()
            .setMemoryBandwidthMbs(measured.memoryBandwidthMbs)
            .setThroughputFlops(measured.throughputFlops)
            .setProbeDurationMs(measured.probeDurationMs)
            .build()
        responseObserver.onNext(response)
        responseObserver.onCompleted()
    }

    private fun unimplemented() =
        Status.UNIMPLEMENTED
            .withDescription("no compute engine on this worker yet")
            .asRuntimeException()

    private fun unexpectedOutcome(rpc: String, outcome: EngineOutcome) =
        Status.INTERNAL
            .withDescription("engine returned ${outcome::class.simpleName} from $rpc")
            .asRuntimeException()

    private fun FloatArray.toLeBytes(): ByteArray {
        val bytes = ByteArray(size * 4)
        ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN).asFloatBuffer().put(this)
        return bytes
    }
}
