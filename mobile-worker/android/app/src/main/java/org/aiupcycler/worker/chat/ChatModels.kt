package org.aiupcycler.worker.chat

import java.io.IOException
import java.util.UUID

/**
 * Wire shapes shared by the LAN client and the later chat UI/reducer tasks.
 *
 * These types deliberately contain no Android, OkHttp, or JSON-library types:
 * Task 9 can consume them as stable value objects from a JVM test.
 */

/** The provider switch's three states. `Auto` omits `provider` from the body. */
sealed interface ProviderChoice {
    object Auto : ProviderChoice
    object Local : ProviderChoice
    data class Cloud(val providerId: String) : ProviderChoice
}

/** A chat turn, or one row returned by `GET /api/sessions/:id/messages`. */
data class ChatMessage(
    val role: String,
    val content: String,
    val messageId: Long? = null,
    val sessionId: String? = null,
    val tokenCount: Long? = null,
    val generationTimeSec: Double? = null,
    val createdAt: String? = null,
    /** Identity for optimistic rows, which have no server [messageId] yet. */
    val clientKey: String = UUID.randomUUID().toString(),
) {
    /**
     * Stable LazyColumn key. Server rows are deterministic; optimistic rows
     * keep their UUID through every `reduce` copy/delta.
     */
    val stableId: String
        get() = messageId?.let { "server-$it" } ?: clientKey
}

/** A row from `GET /api/sessions`; message bodies are never attached here. */
data class ChatSession(
    val sessionId: String,
    val title: String,
    val modelId: String,
    val createdAt: String,
)

/** One model from `GET /v1/models`. */
data class ModelEntry(
    val id: String,
    val obj: String = "",
    val created: Long = 0,
    val ownedBy: String = "",
    val meta: ModelMeta? = null,
)

data class ModelMeta(
    val quantization: String = "",
    val fileSizeBytes: Long = 0,
    val layers: Int = 0,
    val recommendedMinRamGb: Double = 0.0,
)

/** One advertised model inside a provider status row. */
data class ProviderModelWire(
    val id: String,
    val label: String,
    val supportsVision: Boolean = false,
)

/** Credential-free provider status from the bare-array `GET /api/providers`. */
data class ProviderStatus(
    val id: String,
    val name: String,
    val configured: Boolean,
    val models: List<ProviderModelWire> = emptyList(),
    val supportsVision: Boolean = false,
    val supportsTools: Boolean = false,
)

/** Tagged progress shape from `GET /api/devices/:id/provision`. */
sealed interface ProvisionState {
    object Idle : ProvisionState
    data class Running(val bytesDone: Long, val totalBytes: Long) : ProvisionState
    data class Done(val bytes: Long) : ProvisionState
    data class Failed(val message: String) : ProvisionState
}

/**
 * One content-addressed image returned by `POST /api/media`.
 *
 * Wire field names are snake_case; this value type stays Android/JSON free so
 * the payload contract can be checked by a JVM test.
 */
data class MediaRef(
    val mediaHash: String,
    val mimeType: String,
    val fileName: String,
    val byteCount: Long,
)

/** The request the LAN client sends to `/v1/chat/completions`. */
data class ChatRequest(
    val sessionId: String,
    val model: String,
    val provider: ProviderChoice = ProviderChoice.Auto,
    val history: List<ChatMessage> = emptyList(),
    /** Attached to the outgoing user turn only; prior history stays text. */
    val attachments: List<MediaRef> = emptyList(),
)

/** Where a completed turn actually ran. */
sealed interface Source {
    object Local : Source

    /**
     * The serial is kept in the wire form (`adb:<serial>`) so the badge can
     * strip the prefix itself; [displaySerial] is the human-readable part.
     */
    data class Worker(val serial: String) : Source {
        val displaySerial: String
            get() = serial.removePrefix("adb:")
    }

    data class Provider(val id: String) : Source

    companion object {
        /** `local`, `adb:<serial>`, or a provider id (A's telemetry contract). */
        fun fromWire(value: String?): Source = when {
            value.isNullOrBlank() || value.equals("local", ignoreCase = true) -> Local
            value.startsWith("adb:") -> Worker(value)
            else -> Provider(value)
        }
    }
}

/** Named failure modes; no error path is an untyped "handle errors" branch. */
sealed class LanError(open val message: String) {
    data class Unauthorized(val reason: String = "unauthorized") : LanError("unauthorized")
    object NotFound : LanError("not found")
    object Offline : LanError("offline")
    object MalformedResponse : LanError("malformed response")
    data class Server(val code: Int, val reason: String = "http $code") : LanError(reason)
}

/** Carries a named [LanError] through the synchronous and streaming APIs. */
class LanException(val error: LanError) : IOException(error.message)
