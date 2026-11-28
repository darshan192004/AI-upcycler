package org.aiupcycler.worker.chat

import java.net.SocketTimeoutException
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
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/**
 * Device-free contract for the LAN request layer (plan task 8).
 *
 * Every value here is synthetic: no real bearer token or provider key appears
 * in a fixture, and no request body is ever logged by the client.
 */
class LanRequestBuilderTest {

    private val endpoint = LanEndpoint.parse("192.168.1.20:8090")
    private val history = listOf(
        ChatMessage(role = "user", content = "first question"),
        ChatMessage(role = "assistant", content = "first answer"),
    )

    @Test
    fun endpointParsesHostAndPort() {
        assertEquals("192.168.1.20", endpoint.host)
        assertEquals(8090, endpoint.port)
    }

    @Test
    fun endpointRejectsEmptyHostWithExactMessage() {
        val error = runCatching { LanEndpoint.parse(":8090") }.exceptionOrNull()
        assertTrue(error is IllegalArgumentException)
        assertEquals("LAN endpoint host must not be empty", error?.message)
    }

    @Test
    fun endpointRejectsZeroAndOutOfRangePortsWithExactMessage() {
        val zero = runCatching { LanEndpoint.parse("192.168.1.20:0") }.exceptionOrNull()
        assertTrue(zero is IllegalArgumentException)
        assertEquals("LAN endpoint port must be between 1 and 65535", zero?.message)

        val tooLarge = runCatching { LanEndpoint.parse("192.168.1.20:65536") }.exceptionOrNull()
        assertTrue(tooLarge is IllegalArgumentException)
        assertEquals("LAN endpoint port must be between 1 and 65535", tooLarge?.message)
    }

    @Test
    fun endpointRejectsUserinfoWithExactMessage() {
        val error = runCatching {
            LanEndpoint.parse("user:pass@192.168.1.20:8090")
        }.exceptionOrNull()
        assertTrue(error is IllegalArgumentException)
        assertEquals("LAN endpoint must not contain userinfo", error?.message)
    }

    @Test
    fun endpointUsesOnlyLanHostAndLanPortPreferenceKeys() {
        val storage = RecordingEndpointStorage()
        val store = LanEndpointStore(storage)
        assertNull(store.read())

        store.save(endpoint)

        assertEquals(
            mapOf("lan_host" to "192.168.1.20", "lan_port" to 8090),
            storage.written,
        )
        assertEquals(endpoint, store.read())
        assertFalse(storage.written.keys.any { it.contains("token", ignoreCase = true) })
    }

    @Test
    fun chatBodyMarksStreamAndSessionAndOmitsAutoProvider() {
        val body = JSONObject(
            LanRequestBuilder.chatBody("s-test-1", "local-model", ProviderChoice.Auto, history),
        )

        assertTrue(body.getBoolean("stream"))
        assertEquals("s-test-1", body.getString("session_id"))
        assertEquals("local-model", body.getString("model"))
        assertFalse(body.has("provider"))
        val messages = body.getJSONArray("messages")
        assertEquals(2, messages.length())
        assertEquals("user", messages.getJSONObject(0).getString("role"))
        assertEquals("first question", messages.getJSONObject(0).getString("content"))
    }

    @Test
    fun chatBodySendsLocalAndExplicitCloudProviderChoices() {
        val local = JSONObject(
            LanRequestBuilder.chatBody("s-test-1", "m", ProviderChoice.Local, history),
        )
        val cloud = JSONObject(
            LanRequestBuilder.chatBody("s-test-1", "m", ProviderChoice.Cloud("openai"), history),
        )

        assertEquals("local", local.getString("provider"))
        assertEquals("openai", cloud.getString("provider"))
    }

    @Test
    fun authHeaderUsesBearerAndRejectsAnEmptyToken() {
        assertEquals("Bearer token-test-1", LanRequestBuilder.authHeader("token-test-1"))

        val error = runCatching { LanRequestBuilder.authHeader("  ") }.exceptionOrNull()
        assertTrue(error is IllegalStateException)
        assertEquals("LAN token missing; pair again", error?.message)
    }

    @Test
    fun sseParserSplitsCrlfIgnoresHeartbeatsEmitsDeltasAndFinalSource() {
        val sse = buildString {
            append(": keep-alive\r\n\r\n")
            append("data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\r\n\r\n")
            append("data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\r\n\r\n")
            append("data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}],\"source\":\"adb:serial-1\"}\r\n\r\n")
            append("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n")
            append("data: [DONE]\r\n\r\n")
        }

        val events = SseParser().feed(sse)

        assertEquals(listOf("Hel", "lo"), events.mapNotNull { it.content })
        val done = events.last { it.done }
        assertEquals(Source.Worker("adb:serial-1"), done.source)
    }

    @Test
    fun sourceValuesMapToLocalWorkerAndProvider() {
        assertEquals(Source.Local, Source.fromWire("local"))
        assertEquals(Source.Local, Source.fromWire(null))
        assertEquals(Source.Worker("adb:serial-9"), Source.fromWire("adb:serial-9"))
        assertEquals(Source.Provider("gemini"), Source.fromWire("gemini"))
    }

    @Test
    fun errorsMapToNamedTypes() {
        assertEquals(LanError.Unauthorized("unauthorized"), LanRequestBuilder.errorFor(401))
        assertEquals(LanError.NotFound, LanRequestBuilder.errorFor(404))
        assertEquals(LanError.Offline, LanRequestBuilder.errorFor(SocketTimeoutException("timed out")))
        assertEquals(
            LanError.MalformedResponse,
            LanRequestBuilder.errorFor(org.json.JSONException("bad json")),
        )
    }

    @Test
    fun clientSendsBearerTokenOnEveryRequestAndParsesWireShapes() {
        val secrets = MemorySecretStore()
        secrets.write(SecretStore.LAN_TOKEN_KEY, "token-test-client")
        val factory = ScriptedCallFactory(
            { FakeCall(it).apply { response = jsonResponse(200, """{"sessions":[{"session_id":"s-test-1","title":"hi","model_id":"m","created_at":"now"}]}""", it) } },
            { FakeCall(it).apply { response = jsonResponse(200, """{"messages":[{"message_id":7,"session_id":"s-test-1","role":"user","content":"hi","token_count":2,"generation_time_sec":0.0,"created_at":"now"}]}""", it) } },
            { FakeCall(it).apply { response = jsonResponse(200, """{"object":"list","data":[{"id":"m1","object":"model","created":1,"owned_by":"local"}]}""", it) } },
            { FakeCall(it).apply { response = jsonResponse(200, """[{"id":"openai","name":"OpenAI","configured":true,"models":[{"id":"gpt-test","label":"GPT test","supports_vision":false}],"supports_vision":false,"supports_tools":true}]""", it) } },
            { FakeCall(it).apply { response = jsonResponse(200, """{"id":"openai","name":"OpenAI","configured":true,"models":[],"supports_vision":false,"supports_tools":false}""", it) } },
            { FakeCall(it).apply { response = jsonResponse(200, """{"id":"openai","name":"OpenAI","configured":false,"models":[],"supports_vision":false,"supports_tools":false}""", it) } },
            { FakeCall(it).apply { response = jsonResponse(200, """{"state":"running","bytes_done":10,"total_bytes":20}""", it) } },
        )
        val client = LanClient(endpoint, secrets, factory)

        assertEquals("s-test-1", client.listSessions().single().sessionId)
        assertEquals(7L, client.listMessages("s-test-1").single().messageId)
        assertEquals("m1", client.listModels().single().id)
        assertEquals("openai", client.listProviders().single().id)
        client.putProviderKey("openai", "sk-test-client")
        client.deleteProviderKey("openai")
        assertEquals(ProvisionState.Running(10, 20), client.provisionState("node-test-1"))

        assertEquals(7, factory.requests.size)
        factory.requests.forEach {
            assertEquals("Bearer token-test-client", it.header("Authorization"))
        }
        assertEquals("http://192.168.1.20:8090/api/sessions", factory.requests[0].url.toString())
        assertEquals("http://192.168.1.20:8090/api/sessions/s-test-1/messages", factory.requests[1].url.toString())
        assertEquals("http://192.168.1.20:8090/v1/models", factory.requests[2].url.toString())
        assertEquals("http://192.168.1.20:8090/api/providers", factory.requests[3].url.toString())
        assertEquals("http://192.168.1.20:8090/api/settings/secrets/openai", factory.requests[4].url.toString())
        assertEquals("http://192.168.1.20:8090/api/settings/secrets/openai", factory.requests[5].url.toString())
        assertEquals("http://192.168.1.20:8090/api/devices/node-test-1/provision", factory.requests[6].url.toString())
        assertEquals("""{"value":"sk-test-client"}""", factory.requests[4].bodyUtf8())
        assertEquals("DELETE", factory.requests[5].method)
    }

    @Test
    fun clientMapsHttpAndTransportErrors() {
        fun factory(vararg calls: FakeCall) = ScriptedCallFactory(calls.map { call -> { _: Request -> call } })
        val secrets = MemorySecretStore().apply {
            write(SecretStore.LAN_TOKEN_KEY, "token-test-errors")
        }

        val unauthorizedFactory = factory(FakeCall(requestFor("sessions")).apply {
            response = jsonResponse(401, """{"error":{"message":"unauthorized"}}""", requestFor("sessions"))
        })
        assertEquals(
            LanError.Unauthorized("unauthorized"),
            errorOf { LanClient(endpoint, secrets, unauthorizedFactory).listSessions() },
        )

        val notFoundFactory = factory(FakeCall(requestFor("sessions")).apply {
            response = jsonResponse(404, """{"error":{"message":"session missing"}}""", requestFor("sessions"))
        })
        assertEquals(
            LanError.NotFound,
            errorOf { LanClient(endpoint, secrets, notFoundFactory).listSessions() },
        )

        val timeoutFactory = factory(FakeCall(requestFor("models")).apply {
            failure = SocketTimeoutException("socket timed out")
        })
        assertEquals(
            LanError.Offline,
            errorOf { LanClient(endpoint, secrets, timeoutFactory).listModels() },
        )

        val malformedFactory = factory(FakeCall(requestFor("sessions")).apply {
            response = jsonResponse(200, "{not json", requestFor("sessions"))
        })
        assertEquals(
            LanError.MalformedResponse,
            errorOf { LanClient(endpoint, secrets, malformedFactory).listSessions() },
        )
    }

    @Test
    fun defaultClientUsesTheDocumentedTimeouts() {
        val client = LanClient.defaultCallFactory() as okhttp3.OkHttpClient
        assertEquals(5_000, client.connectTimeoutMillis)
        assertEquals(0, client.readTimeoutMillis)
        assertEquals(0, client.callTimeoutMillis)
    }

    @Test
    fun streamChatParsesDeltasAndReturnsACancellableCall() {
        val secrets = MemorySecretStore().apply {
            write(SecretStore.LAN_TOKEN_KEY, "token-test-stream")
        }
        val factory = ScriptedCallFactory({ request: Request ->
            FakeCall(request).apply {
                response = jsonResponse(
                    200,
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\r\n\r\n" +
                        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}],\"source\":\"local\"}\r\n\r\n" +
                        "data: [DONE]\r\n\r\n",
                    request,
                )
            }
        })
        val deltas = mutableListOf<String>()
        var source: Source? = null
        var errors = 0

        val call = LanClient(endpoint, secrets, factory).streamChat(
            ChatRequest("s-test-stream", "m", ProviderChoice.Auto, history),
            onDelta = { deltas += it },
            onDone = { source = it },
            onError = { errors += 1 },
        )

        val fake = factory.calls.single()
        fake.callback!!.onResponse(fake, fake.response!!)
        assertEquals(listOf("Hel", "lo"), deltas)
        assertEquals(Source.Local, source)
        assertEquals(0, errors)
        assertEquals("Bearer token-test-stream", factory.requests.single().header("Authorization"))

        call.cancel()
        assertTrue(call.isCanceled())
    }

    private fun requestFor(path: String): Request = Request.Builder()
        .url(endpoint.baseUrl() + "/" + path)
        .build()

    private fun jsonResponse(code: Int, body: String, request: Request): Response = Response.Builder()
        .request(request)
        .protocol(Protocol.HTTP_1_1)
        .code(code)
        .message("test response")
        .body(ResponseBody.create("application/json".toMediaType(), body))
        .build()

    private fun errorOf(block: () -> Unit): LanError = try {
        block()
        fail("expected a LanException")
        LanError.MalformedResponse
    } catch (error: LanException) {
        error.error
    }

    private class RecordingEndpointStorage : LanEndpointStorage {
        val written = linkedMapOf<String, Any>()

        override fun readString(key: String, defaultValue: String?): String? = written[key] as? String
        override fun readInt(key: String, defaultValue: Int): Int = written[key] as? Int ?: defaultValue
        override fun writeString(key: String, value: String) {
            written[key] = value
        }
        override fun writeInt(key: String, value: Int) {
            written[key] = value
        }
    }

    private class ScriptedCallFactory(
        private val script: List<(Request) -> FakeCall>,
    ) : Call.Factory {
        constructor(vararg script: (Request) -> FakeCall) : this(script.toList())

        val requests = mutableListOf<Request>()
        val calls = mutableListOf<FakeCall>()

        override fun newCall(request: Request): Call {
            requests += request
            val call = script[requests.lastIndex](request)
            calls += call
            return call
        }
    }

    private class FakeCall(private val callRequest: Request) : Call {
        var response: Response? = null
        var failure: Throwable? = null
        var callback: Callback? = null
        private var canceled = false
        private var executed = false

        override fun request(): Request = callRequest

        override fun execute(): Response {
            executed = true
            failure?.let { throw it }
            return response ?: error("no scripted response")
        }

        override fun enqueue(responseCallback: Callback) {
            executed = true
            callback = responseCallback
        }

        override fun cancel() {
            canceled = true
        }

        override fun isExecuted(): Boolean = executed
        override fun isCanceled(): Boolean = canceled
        override fun timeout(): Timeout = Timeout.NONE
        override fun clone(): Call = FakeCall(callRequest).apply {
            response = this@FakeCall.response
            failure = this@FakeCall.failure
        }
    }

    private fun Request.bodyUtf8(): String? {
        val body = body ?: return null
        val buffer = Buffer()
        body.writeTo(buffer)
        return buffer.readUtf8()
    }
}
