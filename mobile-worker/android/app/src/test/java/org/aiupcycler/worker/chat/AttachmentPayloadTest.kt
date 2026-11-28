package org.aiupcycler.worker.chat

import java.io.IOException
import okhttp3.Call
import okhttp3.Callback
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.Response
import okhttp3.ResponseBody
import okio.Buffer
import okio.Timeout
import org.aiupcycler.worker.chat.secrets.MemorySecretStore
import org.aiupcycler.worker.chat.secrets.SecretStore
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Device-free contract for the Task 11 attachment payload.
 *
 * Synthetic hashes and tokens only. This test pins the wire shape that the
 * multipart upload and the part-array send must agree on.
 */
class AttachmentPayloadTest {
    private val endpoint = LanEndpoint.parse("192.168.1.20:8090")
    private val ref = MediaRef(
        mediaHash = "b".repeat(64),
        mimeType = "image/png",
        fileName = "a.png",
        byteCount = 9L,
    )

    @Test
    fun zeroAttachmentsKeepThePlainStringContent() {
        val history = listOf(ChatMessage(role = "user", content = "look"))
        val body = JSONObject(LanRequestBuilder.chatBody("s-1", "m", ProviderChoice.Auto, history))

        val content = body.getJSONArray("messages").getJSONObject(0).get("content")
        assertTrue(content is String)
        assertEquals("look", content)
    }

    @Test
    fun attachmentsSendAPartArrayOnlyOnTheOutgoingTurn() {
        val history = listOf(
            ChatMessage(role = "user", content = "earlier"),
            ChatMessage(role = "user", content = "look"),
        )
        val body = JSONObject(
            LanRequestBuilder.chatBody(
                sessionId = "s-1",
                model = "m",
                provider = ProviderChoice.Auto,
                history = history,
                attachments = listOf(ref),
            ),
        )
        val messages = body.getJSONArray("messages")

        assertTrue(messages.getJSONObject(0).get("content") is String)
        val parts = messages.getJSONObject(1).getJSONArray("content")
        assertEquals(2, parts.length())
        assertEquals("text", parts.getJSONObject(0).getString("type"))
        assertEquals("look", parts.getJSONObject(0).getString("text"))
        val image = parts.getJSONObject(1)
        assertEquals("image", image.getString("type"))
        assertEquals(ref.mediaHash, image.getString("media_hash"))
        assertEquals(ref.mimeType, image.getString("mime_type"))
        assertEquals(ref.fileName, image.getString("file_name"))
        assertEquals(ref.byteCount, image.getLong("byte_count"))
        assertFalse(body.has("provider"))
    }

    @Test
    fun uploadPostsMultipartFileWithBearerAndParsesTheRef() {
        val secrets = MemorySecretStore().apply {
            write(SecretStore.LAN_TOKEN_KEY, "token-attachment-test")
        }
        val factory = ScriptedCallFactory { request ->
            FakeCall(request).apply {
                response = response(
                    request,
                    200,
                    """{"media_hash":"${ref.mediaHash}","mime_type":"image/png","file_name":"a.png","byte_count":9}""",
                )
            }
        }
        val png = byteArrayOf(0x89.toByte(), 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00)

        val result = LanClient(endpoint, secrets, factory).uploadImage("a.png", "image/png", png)

        assertEquals(ref, result)
        val request = factory.requests.single()
        assertEquals("POST", request.method)
        assertEquals("http://192.168.1.20:8090/api/media", request.url.toString())
        assertEquals("Bearer token-attachment-test", request.header("Authorization"))
        assertTrue(request.body!!.contentType()!!.toString().startsWith("multipart/form-data"))
        val body = Buffer().also { request.body!!.writeTo(it) }.readUtf8()
        assertTrue(body.contains("""name="file"; filename="a.png""""))
        assertTrue(body.contains("Content-Type: image/png"))
        assertFalse(body.contains("token-attachment-test"))
    }

    @Test
    fun gateProbeIsTrueOnlyWhenTheMediaRouteExists() {
        val secrets = MemorySecretStore().apply {
            write(SecretStore.LAN_TOKEN_KEY, "token-gate-test")
        }
        fun probe(code: Int): Boolean {
            val factory = ScriptedCallFactory { request -> FakeCall(request).apply { response = response(request, code, "") } }
            return LanClient(endpoint, secrets, factory).mediaGateEnabled()
        }

        assertTrue(probe(405))
        assertFalse(probe(404))
        val factory = ScriptedCallFactory { request ->
            FakeCall(request).apply { failure = IOException("offline") }
        }
        assertFalse(LanClient(endpoint, secrets, factory).mediaGateEnabled())
    }

    @Test
    fun configBooleanIsAuthoritativeWithNoRouteProbe() {
        val secrets = gateSecrets()

        fun probe(configBody: String, expected: Boolean) {
            val factory = ScriptedCallFactory { request ->
                when (request.url.encodedPath) {
                    "/api/config" -> FakeCall(request).apply {
                        response = response(request, 200, configBody)
                    }
                    else -> error("unexpected path ${request.url.encodedPath}")
                }
            }
            assertEquals(expected, LanClient(endpoint, secrets, factory).mediaGateEnabled())
            assertEquals(listOf("/api/config"), factory.requests.map { it.url.encodedPath })
        }

        probe("""{"multimodal_enabled":true}""", true)
        probe("""{"multimodal_enabled":false}""", false)
    }

    @Test
    fun configFailuresFallBackToTheMediaRouteProbe() {
        val secrets = gateSecrets()
        val configFailures = listOf(
            "non-2xx" to { request: Request -> response(request, 500, "{}") },
            "malformed" to { request: Request -> response(request, 200, "{\"multimodal_enabled\":") },
            "missing" to { request: Request -> response(request, 200, "{}") },
            "non-boolean" to { request: Request -> response(request, 200, "{\"multimodal_enabled\":\"true\"}") },
        )

        configFailures.forEach { (_, configResponse) ->
            listOf(405 to true, 404 to false).forEach { (optionsCode, expected) ->
                val factory = ScriptedCallFactory { request ->
                    when (request.url.encodedPath) {
                        "/api/config" -> FakeCall(request).apply { response = configResponse(request) }
                        "/api/media" -> FakeCall(request).apply {
                            response = response(request, optionsCode, "")
                        }
                        else -> error("unexpected path ${request.url.encodedPath}")
                    }
                }
                assertEquals(expected, LanClient(endpoint, secrets, factory).mediaGateEnabled())
                assertEquals(
                    listOf("/api/config", "/api/media"),
                    factory.requests.map { it.url.encodedPath },
                )
            }
        }
    }

    @Test
    fun totalTransportFailureFailsClosedAfterBothCalls() {
        val secrets = gateSecrets()
        val factory = ScriptedCallFactory { request ->
            FakeCall(request).apply { failure = IOException("offline") }
        }

        assertFalse(LanClient(endpoint, secrets, factory).mediaGateEnabled())
        assertEquals(
            listOf("/api/config", "/api/media"),
            factory.requests.map { it.url.encodedPath },
        )
    }

    @Test
    fun attachmentEventsAppendRemoveAndClearWithoutMutatingPriorState() {
        val initial = ChatUiState()
        val added = reduce(initial, ChatEvent.AttachmentAdded(ref))
        val removed = reduce(added, ChatEvent.AttachmentRemoved(ref.mediaHash))

        assertEquals(listOf(ref), added.attachments)
        assertEquals(emptyList<MediaRef>(), removed.attachments)
        assertEquals(emptyList<MediaRef>(), initial.attachments)
        assertEquals(
            emptyList<MediaRef>(),
            reduce(added, ChatEvent.AttachmentsCleared).attachments,
        )
        assertTrue(reduce(initial, ChatEvent.MultimodalGateUpdated(true)).multimodalEnabled)
        assertFalse(initial.multimodalEnabled)
    }

    private fun gateSecrets() = MemorySecretStore().apply {
        write(SecretStore.LAN_TOKEN_KEY, "token-gate-test")
    }

    private fun response(request: Request, code: Int, body: String): Response = Response.Builder()
        .request(request)
        .protocol(Protocol.HTTP_1_1)
        .code(code)
        .message("test response")
        .body(ResponseBody.create("application/json".toMediaType(), body))
        .build()

    private class ScriptedCallFactory(
        private val create: (Request) -> FakeCall,
    ) : Call.Factory {
        val requests = mutableListOf<Request>()

        override fun newCall(request: Request): Call {
            requests += request
            return create(request)
        }
    }

    private class FakeCall(private val request: Request) : Call {
        var response: Response? = null
        var failure: Throwable? = null

        override fun request(): Request = request
        override fun execute(): Response {
            failure?.let { throw it }
            return response ?: error("no scripted response")
        }
        override fun enqueue(callback: Callback) = Unit
        override fun cancel() = Unit
        override fun isExecuted(): Boolean = true
        override fun isCanceled(): Boolean = false
        override fun timeout(): Timeout = Timeout.NONE
        override fun clone(): Call = FakeCall(request).apply { response = this@FakeCall.response }
    }
}
