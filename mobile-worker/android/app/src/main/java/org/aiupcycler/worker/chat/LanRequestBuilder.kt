package org.aiupcycler.worker.chat

import java.io.IOException
import java.net.SocketTimeoutException
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

/** Pure request construction, error mapping, and SSE parsing. */
object LanRequestBuilder {
    /** Reads the token at call time; an absent token is never sent as "Bearer ". */
    fun authHeader(token: String?): String {
        val value = token?.trim().orEmpty()
        if (value.isEmpty()) throw IllegalStateException("LAN token missing; pair again")
        return "Bearer $value"
    }

    /**
     * OpenAI-compatible body. `Auto` omits `provider` so the host keeps its
     * default routing (which also preserves host-only tool behaviour).
     */
    fun chatBody(
        sessionId: String,
        model: String,
        provider: ProviderChoice,
        history: List<ChatMessage>,
        attachments: List<MediaRef> = emptyList(),
    ): String {
        val body = JSONObject()
        body.put("model", model)
        body.put("stream", true)
        body.put("session_id", sessionId)

        val messages = JSONArray()
        history.forEachIndexed { index, message ->
            val outgoing = index == history.lastIndex &&
                message.role.equals("user", ignoreCase = true) &&
                attachments.isNotEmpty()
            val content: Any = if (outgoing) contentParts(message.content, attachments) else message.content
            messages.put(
                JSONObject()
                    .put("role", message.role)
                    .put("content", content),
            )
        }
        body.put("messages", messages)

        when (provider) {
            ProviderChoice.Auto -> Unit
            ProviderChoice.Local -> body.put("provider", "local")
            is ProviderChoice.Cloud -> body.put("provider", provider.providerId)
        }
        return body.toString()
    }

    /**
     * Desktop parity: text-only turns keep a plain string. With attachments,
     * the outgoing user turn becomes `[text part, image parts...]`; no image
     * bytes are ever included, only the hash returned by `/api/media`.
     */
    fun contentParts(text: String, attachments: List<MediaRef>): JSONArray {
        val parts = JSONArray()
        parts.put(JSONObject().put("type", "text").put("text", text))
        attachments.forEach { ref ->
            parts.put(
                JSONObject()
                    .put("type", "image")
                    .put("media_hash", ref.mediaHash)
                    .put("mime_type", ref.mimeType)
                    .put("file_name", ref.fileName)
                    .put("byte_count", ref.byteCount),
            )
        }
        return parts
    }

    fun errorFor(code: Int): LanError = when (code) {
        401 -> LanError.Unauthorized("unauthorized")
        404 -> LanError.NotFound
        else -> LanError.Server(code)
    }

    fun errorFor(error: Throwable): LanError = when (error) {
        is LanException -> error.error
        is SocketTimeoutException -> LanError.Offline
        is JSONException -> LanError.MalformedResponse
        is IOException -> LanError.Offline
        else -> LanError.MalformedResponse
    }
}

/** One parsed SSE data frame. [done] carries the final [source]. */
data class SseEvent(
    val content: String? = null,
    val source: Source? = null,
    val done: Boolean = false,
)

/**
 * Incremental SSE reader. CRLF, LF and bare CR are all line separators;
 * heartbeat comments and non-`data:` fields are ignored. A malformed JSON
 * frame maps to [LanError.MalformedResponse] rather than a silent skip.
 */
class SseParser {
    private val pending = StringBuilder()
    private var lastSource: Source? = null

    fun feed(chunk: String): List<SseEvent> {
        pending.append(chunk)
        val events = mutableListOf<SseEvent>()
        while (true) {
            val text = pending.toString()
            val lf = text.indexOf('\n')
            val cr = text.indexOf('\r')
            val cut = when {
                lf >= 0 && cr >= 0 -> minOf(lf, cr)
                lf >= 0 -> lf
                cr >= 0 -> cr
                else -> break
            }
            var next = cut + 1
            if (text[cut] == '\r' && next < text.length && text[next] == '\n') next += 1
            val line = text.substring(0, cut)
            pending.delete(0, next)
            events += handleLine(line)
        }
        return events
    }

    /** Flushes an unterminated final line, if any. */
    fun finish(): List<SseEvent> {
        if (pending.isEmpty()) return emptyList()
        val line = pending.toString()
        pending.clear()
        return handleLine(line)
    }

    private fun handleLine(raw: String): List<SseEvent> {
        val line = raw.trim()
        if (line.isEmpty() || line.startsWith(":") || !line.startsWith("data:", ignoreCase = true)) {
            return emptyList()
        }
        val data = line.substring(5).trim()
        if (data == "[DONE]") {
            return listOf(SseEvent(source = lastSource, done = true))
        }
        if (data.isEmpty()) return emptyList()

        val frame = try {
            JSONObject(data)
        } catch (_: JSONException) {
            throw LanException(LanError.MalformedResponse)
        }

        val delta = frame.optJSONArray("choices")?.optJSONObject(0)?.optJSONObject("delta")
        val sourceWire = when {
            frame.has("source") -> frame.optString("source")
            delta != null && delta.has("source") -> delta.optString("source")
            else -> null
        }
        val source = sourceWire?.let { Source.fromWire(it) }
        if (source != null) lastSource = source

        val content = delta?.optString("content")?.takeIf { it.isNotEmpty() }
        return listOf(SseEvent(content = content, source = source))
    }
}
