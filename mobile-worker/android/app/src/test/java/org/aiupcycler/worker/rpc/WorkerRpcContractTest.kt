package org.aiupcycler.worker.rpc

import aiupcycler.rpc.ClusterRpc.BeginModelPushRequest
import aiupcycler.rpc.ClusterRpc.ExecuteForwardRequest
import aiupcycler.rpc.ClusterRpc.FinishModelPushRequest
import aiupcycler.rpc.ClusterRpc.HandshakeRequest
import aiupcycler.rpc.ClusterRpc.HealthRequest
import aiupcycler.rpc.ClusterRpc.LoadLayersRequest
import aiupcycler.rpc.ClusterRpc.ModelChunk
import aiupcycler.rpc.ClusterRpc.PushModelChunksResponse
import aiupcycler.rpc.ClusterRpc.TensorBuffer
import aiupcycler.rpc.ClusterRpc.UnloadLayersRequest
import aiupcycler.rpc.WorkerServiceGrpc
import com.google.protobuf.ByteString
import io.grpc.ManagedChannel
import io.grpc.Server
import io.grpc.Status
import io.grpc.StatusRuntimeException
import io.grpc.inprocess.InProcessChannelBuilder
import io.grpc.inprocess.InProcessServerBuilder
import io.grpc.stub.StreamObserver
import org.aiupcycler.worker.engine.WorkerEngine
import org.aiupcycler.worker.metrics.DeviceMetrics
import org.aiupcycler.worker.model.ModelFile
import org.aiupcycler.worker.model.ModelReceiver
import org.aiupcycler.worker.model.ReceiveStart
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Runs the real [WorkerRpcService] over an in-process gRPC transport and calls
 * it through the generated stub.
 *
 * This is the device-free guard on the wire contract. It proves:
 * - the service implements `aiupcycler.rpc.WorkerService` and the metadata
 *   replies carry the fields the master reads (`core/src/rpc/client.rs`);
 * - without an engine, the compute RPCs surface the honest `UNIMPLEMENTED`
 *   of spec 2026-10-19 android-compute-rpcs D-A4 (a phone that has not been
 *   side-loaded a GGUF serves metadata only - never fake compute);
 * - with real engines wired, a full split lifecycle runs over the wire
 *   (D-A6): two services own complementary ranges of the committed 4-layer
 *   fixture, the stage-0 activation threads through both exactly like
 *   `core/tests/distributed.rs` drives its mock pair, and both unload
 *   reporting the bytes they allocated;
 * - the wire-only refusals the Kotlin engine cannot see - missing activation
 *   tensor and non-FP32 dtype - are refused `INVALID_ARGUMENT` at the
 *   wrapper in the Rust engine's wording (D-A2b).
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
    fun computeRpcsAreUnimplementedWithoutAnEngine() {
        // D-A4: absent engine is a legitimate configuration (nothing side-
        // loaded yet), and the master must see it as "cannot compute here".
        assertEquals(
            Status.Code.UNIMPLEMENTED,
            statusOf { stub.loadLayers(LoadLayersRequest.getDefaultInstance()) }.code,
        )
        assertEquals(
            Status.Code.UNIMPLEMENTED,
            statusOf { stub.executeForward(ExecuteForwardRequest.getDefaultInstance()) }.code,
        )
        assertEquals(
            Status.Code.UNIMPLEMENTED,
            statusOf { stub.unloadLayers(UnloadLayersRequest.getDefaultInstance()) }.code,
        )
    }

    @Test
    fun splitLifecycleRunsAcrossTwoEngineBackedServices() {
        val engineA = WorkerEngine.open(fixture("tiny-untied-f32.gguf"))
        val engineB = WorkerEngine.open(fixture("tiny-untied-f32.gguf"))
        try {
            withService(WorkerRpcService(identity, metrics, engineA)) { workerA ->
                withService(WorkerRpcService(identity, metrics, engineB)) { workerB ->
                    // Worker A owns the low half: token_embd + blocks 0..=1.
                    val loadA = workerA.loadLayers(loadRequest(0, 1))
                    assertTrue(loadA.errorMessage, loadA.success)
                    // Worker B owns the high half: blocks 2..=3 + untied head.
                    val loadB = workerB.loadLayers(loadRequest(2, 3))
                    assertTrue(loadB.errorMessage, loadB.success)
                    // Byte accounting survives the wire (engine.rs table).
                    assertEquals(361_728L, loadA.allocatedRamBytes)
                    assertEquals(361_984L, loadB.allocatedRamBytes)

                    // Stage 0: token ids in, hidden out.
                    val stage0 = workerA.executeForward(
                        forwardRequest(
                            "split",
                            0,
                            1,
                            tensor(longArrayOf(1, 3), floatArrayOf(1f, 3f, 2f)),
                            0,
                        ),
                    )
                    assertTrue(stage0.success)
                    val hidden = decodeF32(stage0.outputActivationTensor)
                    assertArrayEquals(
                        longArrayOf(3, 64),
                        stage0.outputActivationTensor.shapeList.toLongArray(),
                    )

                    // Handoff: A's output is B's input; B's range is mid+head.
                    val head = workerB.executeForward(
                        forwardRequest("split", 2, 3, tensor(longArrayOf(3, 64), hidden), 0),
                    )
                    assertTrue(head.success)
                    assertArrayEquals(
                        longArrayOf(3, 256),
                        head.outputActivationTensor.shapeList.toLongArray(),
                    )

                    // An engine refusal rides through as success=false with no
                    // tensor - the Rust engine's wire shape for refusals.
                    val wrongRange = workerB.executeForward(
                        forwardRequest("split", 0, 1, tensor(longArrayOf(3, 64), hidden), 0),
                    )
                    assertFalse(wrongRange.success)
                    assertFalse(wrongRange.hasOutputActivationTensor())

                    // Unload reports exactly what load allocated, both sides.
                    assertEquals(361_984L, workerB.unloadLayers(unloadRequest()).freedRamBytes)
                    assertEquals(361_728L, workerA.unloadLayers(unloadRequest()).freedRamBytes)
                }
            }
        } finally {
            engineA.close()
            engineB.close()
        }
    }

    @Test
    fun forwardWrapperRefusesMissingAndNonFp32Tensors() {
        // The wrapper checks precede engine delegation, so an engine that
        // never loaded anything still proves the D-A2b refusals.
        val engine = WorkerEngine.open(fixture("tiny-untied-f32.gguf"))
        try {
            withService(WorkerRpcService(identity, metrics, engine)) { worker ->
                val missing = statusOf {
                    worker.executeForward(
                        ExecuteForwardRequest.newBuilder().setSessionId("s").build(),
                    )
                }
                assertEquals(Status.Code.INVALID_ARGUMENT, missing.code)
                assertEquals("missing activation_tensor", missing.description)

                val fp16 = tensor(longArrayOf(1, 2), floatArrayOf(1f, 2f)).toBuilder()
                    .setDataType(TensorBuffer.DataType.FP16)
                    .build()
                val wrongDtype = statusOf {
                    worker.executeForward(forwardRequest("s", 0, 3, fp16, 0))
                }
                assertEquals(Status.Code.INVALID_ARGUMENT, wrongDtype.code)
                assertEquals(
                    "activation data_type 1 not supported (FP32 only)",
                    wrongDtype.description,
                )
            }
        } finally {
            engine.close()
        }
    }

    // ---- helpers -------------------------------------------------------------

    private fun fixture(name: String): File {
        val url = javaClass.getResource("/engine/fixtures/$name")
            ?: error("missing fixture resource /engine/fixtures/$name")
        return File(url.toURI())
    }

    /** Spins up a disposable in-process server for [service]. */
    private fun withService(
        service: WorkerRpcService,
        block: (WorkerServiceGrpc.WorkerServiceBlockingStub) -> Unit,
    ) {
        val name = InProcessServerBuilder.generateName()
        val disposableServer = InProcessServerBuilder.forName(name)
            .directExecutor()
            .addService(service)
            .build()
            .start()
        val disposableChannel = InProcessChannelBuilder.forName(name).directExecutor().build()
        try {
            block(WorkerServiceGrpc.newBlockingStub(disposableChannel))
        } finally {
            disposableChannel.shutdownNow()
            disposableServer.shutdownNow()
        }
    }

    /**
     * The async stub, which is the only one that can drive a client-streaming
     * call: grpc-java does not put `PushModelChunks` on the blocking stub.
     */
    private fun withAsyncService(
        service: WorkerRpcService,
        block: (WorkerServiceGrpc.WorkerServiceStub) -> Unit,
    ) {
        val name = InProcessServerBuilder.generateName()
        val disposableServer = InProcessServerBuilder.forName(name)
            .directExecutor()
            .addService(service)
            .build()
            .start()
        val disposableChannel = InProcessChannelBuilder.forName(name).directExecutor().build()
        try {
            block(WorkerServiceGrpc.newStub(disposableChannel))
        } finally {
            disposableChannel.shutdownNow()
            disposableServer.shutdownNow()
        }
    }

    /** Drives a client-streaming push to completion and returns its reply. */
    private fun push(
        stub: WorkerServiceGrpc.WorkerServiceStub,
        chunks: List<ModelChunk>,
    ): PushModelChunksResponse {
        val done = CountDownLatch(1)
        var response: PushModelChunksResponse? = null
        var failure: Throwable? = null
        val request = stub.pushModelChunks(
            object : StreamObserver<PushModelChunksResponse> {
                override fun onNext(value: PushModelChunksResponse) {
                    response = value
                }

                override fun onError(t: Throwable) {
                    failure = t
                    done.countDown()
                }

                override fun onCompleted() {
                    done.countDown()
                }
            },
        )
        for (chunk in chunks) {
            if (failure != null) break
            try {
                request.onNext(chunk)
            } catch (e: Throwable) {
                failure = e
                break
            }
        }
        if (failure == null) {
            runCatching { request.onCompleted() }
        }
        assertTrue("push did not complete", done.await(10, TimeUnit.SECONDS))
        failure?.let { throw it }
        return response ?: throw AssertionError("push returned no response")
    }

    private fun loadRequest(start: Int, end: Int) = LoadLayersRequest.newBuilder()
        .setModelId("tiny-untied-f32")
        .setStartLayer(start)
        .setEndLayer(end)
        .build()

    private fun unloadRequest() = UnloadLayersRequest.newBuilder()
        .setModelId("tiny-untied-f32")
        .build()

    private fun forwardRequest(
        session: String,
        start: Int,
        end: Int,
        buffer: TensorBuffer,
        position: Int,
    ) = ExecuteForwardRequest.newBuilder()
        .setSessionId(session)
        .setStartLayer(start)
        .setEndLayer(end)
        .setActivationTensor(buffer)
        .setSequencePosition(position)
        .build()

    private fun tensor(shape: LongArray, values: FloatArray) = TensorBuffer.newBuilder()
        .addAllShape(shape.asList())
        .setDataType(TensorBuffer.DataType.FP32)
        .setRawData(ByteString.copyFrom(leBytes(values)))
        .build()

    private fun leBytes(values: FloatArray): ByteArray {
        val bytes = ByteArray(values.size * 4)
        ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN).asFloatBuffer().put(values)
        return bytes
    }

    private fun decodeF32(buffer: TensorBuffer): FloatArray {
        val out = FloatArray(buffer.rawData.size() / 4)
        ByteBuffer.wrap(buffer.rawData.toByteArray())
            .order(ByteOrder.LITTLE_ENDIAN)
            .asFloatBuffer()
            .get(out)
        return out
    }

    private fun statusOf(block: () -> Unit): Status =
        try {
            block()
            throw AssertionError("expected the RPC to fail")
        } catch (e: StatusRuntimeException) {
            e.status
        }

    // --- model push (spec 2026-11-03-model-provisioning) ---------------------

    private class FakeReceiver(
        private val resumeFrom: Long = 0,
        private val failFinish: Boolean = false,
    ) : ModelReceiver {
        var written = resumeFrom
        val appended = mutableListOf<ByteArray>()
        var finishedWith: String? = null

        override fun beginReceive(
            fileName: String,
            totalBytes: Long,
            sha256Hex: String,
        ): ReceiveStart = ReceiveStart.Accepted(resumeFrom)

        override fun appendReceive(offset: Long, data: ByteArray): Long {
            require(offset == written) { "chunk offset $offset does not match $written written" }
            written += data.size
            appended += data
            return written
        }

        override fun finishReceive(sha256Hex: String): ModelFile {
            if (failFinish) throw java.io.IOException("sha256 mismatch")
            finishedWith = sha256Hex
            return ModelFile(File("m.gguf"), "m.gguf", written)
        }
    }

    private fun beginRequest() = BeginModelPushRequest.newBuilder()
        .setModelId("m")
        .setFileName("m.gguf")
        .setTotalBytes(8)
        .setSha256Hex("a".repeat(64))
        .build()

    private fun chunk(offset: Long, bytes: ByteArray) = ModelChunk.newBuilder()
        .setOffset(offset)
        .setData(ByteString.copyFrom(bytes))
        .build()

    @Test
    fun beginModelPushReportsTheResumeOffset() {
        val receiver = FakeReceiver(resumeFrom = 3)
        withService(WorkerRpcService(identity, metrics, receiver = receiver)) { worker ->
            val reply = worker.beginModelPush(beginRequest())

            assertTrue(reply.accepted)
            assertEquals(3L, reply.resumeFromBytes)
        }
    }

    @Test
    fun beginModelPushIsUnimplementedWithoutAReceiver() {
        assertEquals(
            Status.Code.UNIMPLEMENTED,
            statusOf { stub.beginModelPush(beginRequest()) }.code,
        )
    }

    @Test
    fun chunksAreAppendedInOrder() {
        val receiver = FakeReceiver()
        withAsyncService(WorkerRpcService(identity, metrics, receiver = receiver)) { worker ->
            val reply = push(
                worker,
                listOf(
                    chunk(0, ByteArray(3) { 1 }),
                    chunk(3, ByteArray(5) { 2 }),
                ),
            )

            assertTrue(reply.errorMessage, reply.success)
            assertEquals(8L, reply.receivedBytes)
            assertEquals(8, receiver.appended.sumOf { it.size })
        }
    }

    @Test
    fun anOutOfOrderChunkFailsTheStream() {
        val receiver = FakeReceiver()
        withAsyncService(WorkerRpcService(identity, metrics, receiver = receiver)) { worker ->
            val error = try {
                push(
                    worker,
                    listOf(
                        chunk(0, ByteArray(4)),
                        chunk(0, ByteArray(4)),
                    ),
                )
                null
            } catch (e: StatusRuntimeException) {
                e
            }

            assertEquals(Status.Code.INVALID_ARGUMENT, error?.status?.code)
        }
    }

    @Test
    fun finishReportsAVerificationFailureAsUnsuccessful() {
        val receiver = FakeReceiver(failFinish = true)
        withService(WorkerRpcService(identity, metrics, receiver = receiver)) { worker ->
            val reply = worker.finishModelPush(
                FinishModelPushRequest.newBuilder()
                    .setModelId("m")
                    .setSha256Hex("a".repeat(64))
                    .build(),
            )

            assertFalse(reply.success)
            assertTrue(reply.errorMessage.contains("sha256 mismatch"))
        }
    }

    @Test
    fun finishCallsBackWithTheInstalledFile() {
        val receiver = FakeReceiver()
        val provisioned = mutableListOf<ModelFile>()
        withService(
            WorkerRpcService(
                identity,
                metrics,
                receiver = receiver,
                onProvisioned = {
                    provisioned += it
                    true
                },
            ),
        ) { worker ->
            val reply = worker.finishModelPush(
                FinishModelPushRequest.newBuilder()
                    .setModelId("m")
                    .setSha256Hex("a".repeat(64))
                    .build(),
            )

            assertTrue(reply.errorMessage, reply.success)
            assertEquals(listOf("m.gguf"), provisioned.map { it.displayName })
            assertTrue(reply.engineStarted)
            assertEquals("m.gguf", File(reply.modelPath).name)
        }
    }

    @Test
    fun finishWarnsWhenTheEngineWasNotReopened() {
        val receiver = FakeReceiver()
        withService(
            WorkerRpcService(
                identity,
                metrics,
                receiver = receiver,
                onProvisioned = { false },
            ),
        ) { worker ->
            val reply = worker.finishModelPush(
                FinishModelPushRequest.newBuilder()
                    .setModelId("m")
                    .setSha256Hex("a".repeat(64))
                    .build(),
            )

            assertTrue(reply.success)
            assertFalse(reply.engineStarted)
            assertTrue(reply.errorMessage.contains("reopens it"))
        }
    }
}
