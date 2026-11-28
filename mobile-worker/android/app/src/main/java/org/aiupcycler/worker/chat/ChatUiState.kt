package org.aiupcycler.worker.chat

/**
 * Renderable chat state for the task 9 reducers.
 *
 * This file deliberately contains only value types and a pure top-level
 * [reduce]: no Android, OkHttp, JSON, or I/O types appear here, so the whole
 * state machine runs in a JVM unit test. Session identity is always the exact
 * `session_id` string from the LAN wire; message identity is the stable
 * [ChatMessage.messageId], never a list index.
 *
 * [reducedMotion] and [compact] are presentation flags owned by the caller;
 * reducers copy them through unchanged on every event.
 */
data class ChatUiState(
    val endpoint: LanEndpoint? = null,
    val sessions: List<ChatSession> = emptyList(),
    val activeSessionId: String = "",
    val messages: List<ChatMessage> = emptyList(),
    val draft: String = "",
    val busy: Boolean = false,
    val source: Source? = null,
    val models: List<ModelEntry> = emptyList(),
    val providers: List<ProviderStatus> = emptyList(),
    val selectedProvider: ProviderChoice = ProviderChoice.Auto,
    val selectedModel: String = "",
    val provisioning: ProvisionState = ProvisionState.Idle,
    val banner: Banner? = null,
    val reducedMotion: Boolean = false,
    val compact: Boolean = false,
)

/** Every state transition the chat UI can ask the reducer to apply. */
sealed interface ChatEvent {
    data class SessionListLoaded(val sessions: List<ChatSession>) : ChatEvent
    data class Delta(val text: String) : ChatEvent
    data class StreamDone(val source: Source) : ChatEvent
    data class ProvisioningUpdated(val provisioning: ProvisionState) : ChatEvent
    object Unauthorized : ChatEvent
    data class ModelCatalogLoaded(
        val models: List<ModelEntry>,
        val providers: List<ProviderStatus>,
    ) : ChatEvent
}

/** User-facing notices; [blocking] marks a notice that must stop interaction. */
sealed interface Banner {
    val message: String
    val blocking: Boolean get() = false

    data class Unauthorized(
        override val message: String = "LAN credentials rejected; pair again",
    ) : Banner {
        override val blocking: Boolean = true
    }

    data class Offline(
        override val message: String = "LAN host is offline; check the connection",
    ) : Banner

    data class NotFound(
        override val message: String = "LAN route or session was not found",
    ) : Banner

    data class MalformedResponse(
        override val message: String = "LAN host returned an unexpected response",
    ) : Banner

    data class PairingRequired(
        override val message: String = "Pair with the LAN host to continue",
    ) : Banner {
        override val blocking: Boolean = true
    }
}

/**
 * Applies [event] to [state] and returns a new state.
 *
 * Inputs are never mutated: collections are copied and only the affected
 * element is replaced. Presentation flags are carried through every branch.
 */
fun reduce(state: ChatUiState, event: ChatEvent): ChatUiState = when (event) {
    is ChatEvent.SessionListLoaded -> state.copy(sessions = event.sessions.toList())

    is ChatEvent.Delta -> appendDelta(state, event.text)

    is ChatEvent.StreamDone -> state.copy(source = event.source, busy = false)

    is ChatEvent.ProvisioningUpdated -> state.copy(provisioning = event.provisioning)

    ChatEvent.Unauthorized -> state.copy(banner = Banner.Unauthorized())

    is ChatEvent.ModelCatalogLoaded -> mergeCatalog(state, event.models, event.providers)
}

/** Appends to the latest assistant message with an id, not to the last row. */
private fun appendDelta(state: ChatUiState, text: String): ChatUiState {
    if (text.isEmpty()) return state
    val targetIndex = state.messages.indexOfLast {
        it.role.equals("assistant", ignoreCase = true) && it.messageId != null
    }
    if (targetIndex < 0) return state
    return state.copy(
        messages = state.messages.mapIndexed { index, message ->
            if (index == targetIndex) message.copy(content = message.content + text) else message
        },
    )
}

/**
 * Keeps local catalog ids intact and appends one `provider:model` id per
 * advertised model, ordered by provider id then model id. Ids already present
 * locally (or seen earlier in the provider list) are not added again.
 */
private fun mergeCatalog(
    state: ChatUiState,
    models: List<ModelEntry>,
    providers: List<ProviderStatus>,
): ChatUiState {
    val seen = HashSet(models.map { it.id })
    val advertised = providers
        .flatMap { provider -> provider.models.map { provider.id to it.id } }
        .sortedWith(compareBy({ it.first }, { it.second }))
        .mapNotNull { (providerId, modelId) ->
            val id = "$providerId:$modelId"
            if (seen.add(id)) ModelEntry(id = id, ownedBy = providerId) else null
        }
    return state.copy(
        models = models.toList() + advertised,
        providers = providers.toList(),
    )
}
