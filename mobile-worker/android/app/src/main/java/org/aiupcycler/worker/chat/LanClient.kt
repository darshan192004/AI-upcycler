package org.aiupcycler.worker.chat

import java.io.IOException
import java.net.SocketTimeoutException
import java.util.concurrent.TimeUnit
import okhttp3.Call
import okhttp3.Callback
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.MultipartBody
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import org.aiupcycler.worker.chat.secrets.SecretStore
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

private val LAN_JSON = "application/json; charset=utf-8".toMediaType()

/**
 * The six non-secret settings fields accepted by `PUT /api/settings`. There is
 * deliberately no key/token/secret property: provider credentials stay behind
 * `/api/settings/secrets/:id` and are never serialised here.
 */
data class SettingsDto(
    val provider: String?,
    val model: String?,
    val context_tokens: Int,
    val max_tokens: Int,
    val show_source_badges: Boolean,
    val compact_mode: Boolean,
)

/** Host-derived limits plus the human-readable reasons behind them. */
data class SettingsRecommendationDto(
    val context_tokens: Int,
    val max_tokens: Int,
    val reasons: List<String>,
)

internal fun SettingsDto.toJsonObject(): JSONObject = JSONObject().apply {
    put("provider", provider ?: JSONObject.NULL)
    put("model", model ?: JSONObject.NULL)
    put("context_tokens", context_tokens)
    put("max_tokens", max_tokens)
    put("show_source_badges", show_source_badges)
    put("compact_mode", compact_mode)
}

internal fun JSONObject.toSettingsDto(): SettingsDto = SettingsDto(
    provider = optNullableString("provider"),
    model = optNullableString("model"),
    context_tokens = getInt("context_tokens"),
    max_tokens = getInt("max_tokens"),
    show_source_badges = getBoolean("show_source_badges"),
    compact_mode = getBoolean("compact_mode"),
)

internal fun JSONObject.toSettingsRecommendationDto(): SettingsRecommendationDto {
    val values = optJSONArray("reasons")
    return SettingsRecommendationDto(
        context_tokens = getInt("context_tokens"),
        max_tokens = getInt("max_tokens"),
        reasons = if (values == null) {
            emptyList()
        } else {
            (0 until values.length()).map { values.optString(it) }
        },
    )
}

/** Applies a recommendation by replacing only the two limit fields. */
internal fun SettingsDto.withLimitsFrom(
    recommendation: SettingsRecommendationDto,
): SettingsDto = copy(
    context_tokens = recommendation.context_tokens,
    max_tokens = recommendation.max_tokens,
)

private fun JSONObject.optNullableString(key: String): String? =
    if (isNull(key)) null else optString(key).takeIf { it.isNotBlank() }

/**
 * Bearer-authenticated LAN client. The [Call.Factory] is injected so every
 * route can be exercised on the JVM with a scripted call; production uses
 * [defaultCallFactory]. No token or response body is ever logged.
 */
class LanClient(
    private val endpoint: LanEndpoint,
    private val secrets: SecretStore,
    private val callFactory: Call.Factory,
) {
    fun listSessions(): List<ChatSession> {
        val sessions = executeJson("GET", "/api/sessions").optJSONArray("sessions")
            ?: throw LanException(LanError.MalformedResponse)
        return (0 until sessions.length()).map { sessions.getJSONObject(it).toChatSession() }
    }

    fun listMessages(sessionId: String): List<ChatMessage> {
        val messages = executeJson("GET", "/api/sessions/$sessionId/messages")
            .optJSONArray("messages") ?: throw LanException(LanError.MalformedResponse)
        return (0 until messages.length()).map { messages.getJSONObject(it).toChatMessage() }
    }

    fun listModels(): List<ModelEntry> {
        val models = executeJson("GET", "/v1/models").optJSONArray("data")
            ?: throw LanException(LanError.MalformedResponse)
        return (0 until models.length()).map { models.getJSONObject(it).toModelEntry() }
    }

    fun listProviders(): List<ProviderStatus> =
        executeJsonArray("GET", "/api/providers").toProviders()

    fun getJson(path: String): JSONObject = executeJson("GET", path)

    /**
     * Reads the one LAN bearer token from the existing [SecretStore] for the
     * image client. The value is never logged, displayed, or copied elsewhere.
     */
    fun lanToken(): String? = secrets.read(SecretStore.LAN_TOKEN_KEY)

    /** Non-secret host settings; no key, token, or secret field is ever built. */
    fun getSettings(): SettingsDto = getJson("/api/settings").toSettingsDto()

    /** Carries the full value object through; only the caller's changed fields differ. */
    fun putSettings(settings: SettingsDto): SettingsDto =
        executeJson("PUT", "/api/settings", jsonBody(settings.toJsonObject().toString()))
            .toSettingsDto()

    fun getSettingsRecommendations(): SettingsRecommendationDto =
        getJson("/api/settings/recommendations").toSettingsRecommendationDto()

    /** Writes the value to the host's secret store; it is never echoed back. */
    fun putProviderKey(id: String, value: String) {
        val body = JSONObject().put("value", value).toString()
        executeJson("PUT", "/api/settings/secrets/$id", jsonBody(body))
    }

    fun deleteProviderKey(id: String) {
        executeJson("DELETE", "/api/settings/secrets/$id")
    }

    /**
     * Multipart `POST /api/media` with the documented `file` field. The bearer
     * token is added by [request] from the secure store and never logged or
     * written into the body; only the server's content hash is returned.
     */
    fun uploadImage(fileName: String, mimeType: String, bytes: ByteArray): MediaRef {
        val file = bytes.toRequestBody(mimeType.toMediaType())
        val body = MultipartBody.Builder()
            .setType(MultipartBody.FORM)
            .addFormDataPart("file", fileName, file)
            .build()
        return executeJson("POST", "/api/media", body).toMediaRef()
    }

    /**
     * Fail-closed gate check. `/api/config` is authoritative when it provides
     * a boolean `multimodal_enabled`. If that call fails for any reason —
     * transport, non-2xx, malformed JSON, missing or non-boolean field — fall
     * back to the `/api/media` route probe: a present route answers 405 to
     * OPTIONS, an absent one answers 404. Everything else keeps the gate off.
     * No token or response body is logged.
     */
    fun mediaGateEnabled(): Boolean {
        try {
            val enabled = getJson("/api/config").opt("multimodal_enabled") as? Boolean
            if (enabled != null) return enabled
        } catch (_: Exception) {
            // Config is unavailable or invalid; fall through to the route probe.
        }
        val call = callFactory.newCall(request("OPTIONS", "/api/media"))
        val response = try {
            call.execute()
        } catch (_: IOException) {
            return false
        }
        return response.use { it.code == 405 }
    }

    fun provisionState(deviceId: String): ProvisionState =
        executeJson("GET", "/api/devices/$deviceId/provision").toProvisionState()

    /**
     * Starts an SSE turn and returns the [Call] so the UI can cancel it.
     * [onError] is optional for callers that only render the done state.
     */
    fun streamChat(
        request: ChatRequest,
        onDelta: (String) -> Unit,
        onDone: (Source) -> Unit,
        onError: (LanError) -> Unit = {},
    ): Call {
        val body = LanRequestBuilder.chatBody(
            sessionId = request.sessionId,
            model = request.model,
            provider = request.provider,
            history = request.history,
            attachments = request.attachments,
        )
        val call = callFactory.newCall(
            request("POST", "/v1/chat/completions", jsonBody(body)),
        )
        val parser = SseParser()

        fun dispatch(events: List<SseEvent>) {
            events.forEach { event ->
                event.content?.let(onDelta)
                if (event.done) onDone(event.source ?: Source.Local)
            }
        }

        call.enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) {
                onError(LanRequestBuilder.errorFor(e))
            }

            override fun onResponse(call: Call, response: Response) {
                response.use {
                    if (!it.isSuccessful) {
                        onError(LanRequestBuilder.errorFor(it.code))
                        return@use
                    }
                    try {
                        val source = it.body?.source()
                            ?: throw LanException(LanError.MalformedResponse)
                        source.use { buffered ->
                            while (!buffered.exhausted()) {
                                val line = buffered.readUtf8Line() ?: break
                                dispatch(parser.feed(line + "\n"))
                            }
                        }
                        dispatch(parser.finish())
                    } catch (error: LanException) {
                        onError(error.error)
                    } catch (error: IOException) {
                        onError(LanRequestBuilder.errorFor(error))
                    }
                }
            }
        })
        return call
    }

    private fun request(method: String, path: String, body: RequestBody? = null): Request {
        val builder = Request.Builder()
            .url(endpoint.baseUrl() + path)
            .header("Authorization", LanRequestBuilder.authHeader(secrets.read(SecretStore.LAN_TOKEN_KEY)))
            .header("Accept", "application/json")
        if (body != null) {
            builder.method(method, body)
        } else {
            builder.method(method, null)
        }
        return builder.build()
    }

    private fun jsonBody(text: String): RequestBody = text.toRequestBody(LAN_JSON)

    private fun executeJson(method: String, path: String, body: RequestBody? = null): JSONObject {
        val call = callFactory.newCall(request(method, path, body))
        val response = try {
            call.execute()
        } catch (error: SocketTimeoutException) {
            throw LanException(LanError.Offline)
        } catch (error: IOException) {
            throw LanException(LanError.Offline)
        }
        response.use {
            if (!it.isSuccessful) throw LanException(LanRequestBuilder.errorFor(it.code))
            val text = it.body?.string() ?: throw LanException(LanError.MalformedResponse)
            return try {
                JSONObject(text)
            } catch (_: JSONException) {
                throw LanException(LanError.MalformedResponse)
            }
        }
    }

    private fun executeJsonArray(method: String, path: String): JSONArray {
        val call = callFactory.newCall(request(method, path))
        val response = try {
            call.execute()
        } catch (error: SocketTimeoutException) {
            throw LanException(LanError.Offline)
        } catch (error: IOException) {
            throw LanException(LanError.Offline)
        }
        response.use {
            if (!it.isSuccessful) throw LanException(LanRequestBuilder.errorFor(it.code))
            val text = it.body?.string() ?: throw LanException(LanError.MalformedResponse)
            return try {
                JSONArray(text)
            } catch (_: JSONException) {
                throw LanException(LanError.MalformedResponse)
            }
        }
    }

    companion object {
        const val CONNECT_TIMEOUT_MS = 5_000L
        const val READ_TIMEOUT_MS = 0L
        const val CALL_TIMEOUT_MS = 0L

        /** Read timeout zero is deliberate: SSE turns are long-lived. */
        fun defaultCallFactory(): Call.Factory = OkHttpClient.Builder()
            .connectTimeout(CONNECT_TIMEOUT_MS, TimeUnit.MILLISECONDS)
            .readTimeout(READ_TIMEOUT_MS, TimeUnit.MILLISECONDS)
            .callTimeout(CALL_TIMEOUT_MS, TimeUnit.MILLISECONDS)
            .build()
    }
}

private fun JSONObject.toChatSession(): ChatSession = ChatSession(
    sessionId = optString("session_id"),
    title = optString("title"),
    modelId = optString("model_id"),
    createdAt = optString("created_at"),
)

private fun JSONObject.toMediaRef(): MediaRef {
    val hash = optString("media_hash")
    val mime = optString("mime_type")
    val name = optString("file_name")
    val bytes = optLong("byte_count", 0L)
    if (hash.isBlank() || mime.isBlank() || name.isBlank() || bytes <= 0L) {
        throw LanException(LanError.MalformedResponse)
    }
    return MediaRef(mediaHash = hash, mimeType = mime, fileName = name, byteCount = bytes)
}

private fun JSONObject.toChatMessage(): ChatMessage = ChatMessage(
    role = optString("role"),
    content = optString("content"),
    messageId = optLong("message_id", -1L),
    sessionId = optString("session_id").ifEmpty { null },
    tokenCount = optLong("token_count", 0L),
    generationTimeSec = optDouble("generation_time_sec", 0.0),
    createdAt = optString("created_at").ifEmpty { null },
)

private fun JSONObject.toModelEntry(): ModelEntry {
    val meta = optJSONObject("meta")
    return ModelEntry(
        id = optString("id"),
        obj = optString("object"),
        created = optLong("created", 0L),
        ownedBy = optString("owned_by"),
        meta = meta?.let {
            ModelMeta(
                quantization = it.optString("quantization"),
                fileSizeBytes = it.optLong("file_size_bytes", 0L),
                layers = it.optInt("layers", 0),
                recommendedMinRamGb = it.optDouble("recommended_min_ram_gb", 0.0),
            )
        },
    )
}

private fun JSONArray.toProviders(): List<ProviderStatus> = (0 until length()).map { index ->
    val json = getJSONObject(index)
    val models = json.optJSONArray("models")
    ProviderStatus(
        id = json.optString("id"),
        name = json.optString("name"),
        configured = json.optBoolean("configured"),
        models = if (models == null) {
            emptyList()
        } else {
            (0 until models.length()).map { i ->
                models.getJSONObject(i).let { model ->
                    ProviderModelWire(
                        id = model.optString("id"),
                        label = model.optString("label"),
                        supportsVision = model.optBoolean("supports_vision"),
                    )
                }
            }
        },
        supportsVision = json.optBoolean("supports_vision"),
        supportsTools = json.optBoolean("supports_tools"),
    )
}

private fun JSONObject.toProvisionState(): ProvisionState = when (optString("state")) {
    "idle" -> ProvisionState.Idle
    "running" -> ProvisionState.Running(
        bytesDone = optLong("bytes_done", 0L),
        totalBytes = optLong("total_bytes", 0L),
    )
    "done" -> ProvisionState.Done(optLong("bytes", 0L))
    "failed" -> ProvisionState.Failed(optString("message"))
    else -> throw LanException(LanError.MalformedResponse)
}
