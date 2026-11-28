package org.aiupcycler.worker.chat

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
import org.junit.Test

/**
 * Device-free contract for the settings surface used by auto-apply.
 *
 * Every provider/model/token value here is synthetic. The assertions pin the
 * six-field non-secret DTO shape and the get → recommend → put mapping.
 */
class SettingsContractTest {
    private val endpoint = LanEndpoint.parse("192.168.1.20:8090")
    private val fields = setOf(
        "provider",
        "model",
        "context_tokens",
        "max_tokens",
        "show_source_badges",
        "compact_mode",
    )

    @Test
    fun settingsBodyHasExactlySixNonSecretFields() {
        val json = SettingsDto(
            provider = "local-provider",
            model = "model-test",
            context_tokens = 4_096,
            max_tokens = 1_024,
            show_source_badges = true,
            compact_mode = false,
        ).toJsonObject()

        assertEquals(fields, json.keys().asSequence().toSet())
        assertFalse(json.has("api_key"))
        assertFalse(json.has("token"))
        assertFalse(json.has("secret"))
        assertFalse(json.has("provider_key"))
        assertEquals(4_096, json.getInt("context_tokens"))
        assertEquals(1_024, json.getInt("max_tokens"))
        assertTrue(json.getBoolean("show_source_badges"))
        assertFalse(json.getBoolean("compact_mode"))
    }

    @Test
    fun nullOptionalsRoundTripAsJsonNull() {
        val dto = SettingsDto(null, null, 2_048, 512, false, true)
        val json = dto.toJsonObject()

        assertTrue(json.isNull("provider"))
        assertTrue(json.isNull("model"))
        assertEquals(dto, json.toSettingsDto())
        assertEquals(fields, json.keys().asSequence().toSet())
    }

    @Test
    fun recommendationsReplaceOnlyLimits() {
        val current = SettingsDto("openai-test", "model-test", 4_096, 1_024, false, true)
        val recommended = SettingsRecommendationDto(
            context_tokens = 16_384,
            max_tokens = 2_048,
            reasons = listOf("synthetic-memory", "synthetic-worker"),
        )

        val applied = current.withLimitsFrom(recommended)

        assertEquals(16_384, applied.context_tokens)
        assertEquals(2_048, applied.max_tokens)
        assertEquals("openai-test", applied.provider)
        assertEquals("model-test", applied.model)
        assertFalse(applied.show_source_badges)
        assertTrue(applied.compact_mode)
        assertEquals(fields, applied.toJsonObject().keys().asSequence().toSet())
        assertFalse(applied.toJsonObject().has("api_key"))
    }

    @Test
    fun clientUsesSettingsRoutesWithBearerAndNoSecretsInBody() {
        val secrets = MemorySecretStore().apply {
            write(SecretStore.LAN_TOKEN_KEY, "token-settings-test")
        }
        val putBody = JSONObject()
            .put("provider", JSONObject.NULL)
            .put("model", "model-test")
            .put("context_tokens", 16_384)
            .put("max_tokens", 2_048)
            .put("show_source_badges", true)
            .put("compact_mode", false)
            .toString()
        val factory = ScriptedCallFactory(
            { request -> FakeCall(request).apply { response = response(request, 200, """{"provider":null,"model":"model-test","context_tokens":4096,"max_tokens":1024,"show_source_badges":true,"compact_mode":false}""") } },
            { request -> FakeCall(request).apply { response = response(request, 200, """{"context_tokens":16384,"max_tokens":2048,"reasons":["synthetic"]}""") } },
            { request -> FakeCall(request).apply { response = response(request, 200, putBody) } },
        )
        val client = LanClient(endpoint, secrets, factory)

        val current = client.getSettings()
        val recommendation = client.getSettingsRecommendations()
        val applied = client.putSettings(current.withLimitsFrom(recommendation))

        assertNull(current.provider)
        assertEquals("model-test", current.model)
        assertEquals(listOf("synthetic"), recommendation.reasons)
        assertEquals(16_384, recommendation.context_tokens)
        assertEquals(16_384, applied.context_tokens)
        assertEquals(2_048, applied.max_tokens)
        assertEquals(listOf("/api/settings", "/api/settings/recommendations", "/api/settings"),
            factory.requests.map { it.url.encodedPath })
        assertEquals(listOf("GET", "GET", "PUT"), factory.requests.map { it.method })
        factory.requests.forEach {
            assertEquals("Bearer token-settings-test", it.header("Authorization"))
        }
        val written = factory.requests.last().bodyUtf8()!!
        assertEquals(fields, JSONObject(written).keys().asSequence().toSet())
        assertFalse(written.contains("api_key"))
        assertFalse(written.contains("provider_key"))
        assertFalse(written.contains("sk-"))
        assertFalse(written.contains("token-settings-test"))
    }

    private fun response(request: Request, code: Int, body: String): Response = Response.Builder()
        .request(request)
        .protocol(Protocol.HTTP_1_1)
        .code(code)
        .message("test response")
        .body(ResponseBody.create("application/json".toMediaType(), body))
        .build()

    private fun Request.bodyUtf8(): String? {
        val requestBody = body ?: return null
        val buffer = Buffer()
        requestBody.writeTo(buffer)
        return buffer.readUtf8()
    }

    private class ScriptedCallFactory(
        private val script: List<(Request) -> FakeCall>,
    ) : Call.Factory {
        constructor(vararg script: (Request) -> FakeCall) : this(script.toList())

        val requests = mutableListOf<Request>()

        override fun newCall(request: Request): Call {
            requests += request
            return script[requests.lastIndex](request)
        }
    }

    private class FakeCall(private val callRequest: Request) : Call {
        var response: Response? = null

        override fun request(): Request = callRequest
        override fun execute(): Response = response ?: error("no scripted response")
        override fun enqueue(callback: Callback) = Unit
        override fun cancel() = Unit
        override fun isExecuted(): Boolean = true
        override fun isCanceled(): Boolean = false
        override fun timeout(): Timeout = Timeout.NONE
        override fun clone(): Call = FakeCall(callRequest).apply { response = this@FakeCall.response }
    }
}
