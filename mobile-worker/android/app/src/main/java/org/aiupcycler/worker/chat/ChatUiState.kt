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
    /** Composer chips; cleared on send/new session, never sent as image bytes. */
    val attachments: List<MediaRef> = emptyList(),
    /** Host multimodal gate. Fail-closed: off until the host proves it is on. */
    val multimodalEnabled: Boolean = false,
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

    /** A provider switch; the selected model is revalidated against its catalog. */
    data class ProviderSelected(val choice: ProviderChoice) : ChatEvent

    /** A model chosen from the merged local/virtual id list. */
    data class ModelSelected(val modelId: String) : ChatEvent

    data class AttachmentAdded(val ref: MediaRef) : ChatEvent
    data class AttachmentRemoved(val mediaHash: String) : ChatEvent
    object AttachmentsCleared : ChatEvent
    data class MultimodalGateUpdated(val enabled: Boolean) : ChatEvent
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

    data class InvalidAttachment(
        override val message: String = "Invalid image attachment",
    ) : Banner
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

    is ChatEvent.ProviderSelected -> selectProvider(state, event.choice)

    is ChatEvent.ModelSelected -> when (event.modelId) {
        in state.models.map { it.id } -> state.copy(selectedModel = event.modelId)
        else -> state
    }

    is ChatEvent.AttachmentAdded -> state.copy(
        attachments = state.attachments + event.ref,
    )

    is ChatEvent.AttachmentRemoved -> state.copy(
        attachments = state.attachments.filterNot { it.mediaHash == event.mediaHash },
    )

    ChatEvent.AttachmentsCleared -> state.copy(attachments = emptyList())

    is ChatEvent.MultimodalGateUpdated -> state.copy(multimodalEnabled = event.enabled)
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

/**
 * The exact `provider` field value for a chat request. `null` means the key is
 * omitted, which keeps `Auto` routing (and host-only tool behaviour) intact.
 */
fun providerRequestValue(choice: ProviderChoice): String? = when (choice) {
    ProviderChoice.Auto -> null
    ProviderChoice.Local -> "local"
    is ProviderChoice.Cloud -> choice.providerId
}

private val DEFAULT_PROVIDER_NAMES = mapOf(
    "openai" to "OpenAI",
    "anthropic" to "Anthropic",
    "gemini" to "Gemini",
    "kimi" to "Kimi",
)

/**
 * User-facing execution-source label. A provider configured on the host wins
 * over the built-in display name; an id that is neither configured nor known
 * stays unknown rather than guessing a cloud name.
 */
fun sourceBadgeText(
    source: Source?,
    providers: List<ProviderStatus> = emptyList(),
): String = when (source) {
    null -> "Source unknown"
    Source.Local -> "Local engine"
    is Source.Worker -> "Worker ${source.displaySerial}"
    is Source.Provider -> {
        val configured = providers.firstOrNull { it.id.equals(source.id, ignoreCase = true) }?.name
        val name = configured?.takeIf { it.isNotBlank() }
            ?: DEFAULT_PROVIDER_NAMES[source.id.lowercase()]
        if (name.isNullOrBlank()) "Source unknown" else "Cloud $name"
    }
}

/** Exact text rendered for each provisioning state; no index-derived percentage. */
fun provisioningText(state: ProvisionState): String = when (state) {
    ProvisionState.Idle -> ""
    is ProvisionState.Running -> "${state.bytesDone}/${state.totalBytes}"
    is ProvisionState.Done -> "Model ready"
    is ProvisionState.Failed -> state.message
}

/** The progress screen polls only while the host reports a running push. */
fun shouldPollProvisioning(state: ProvisionState): Boolean = state is ProvisionState.Running

/**
 * Model ids that belong to [choice]. Local catalog rows keep their exact ids
 * even when one already contains a colon; virtual cloud rows carry [ModelEntry.ownedBy].
 */
fun availableModelIds(state: ChatUiState, choice: ProviderChoice): List<String> = when (choice) {
    ProviderChoice.Auto -> state.models.map { it.id }
    ProviderChoice.Local -> state.models.filter { it.ownedBy.isBlank() }.map { it.id }
    is ProviderChoice.Cloud -> state.models
        .filter { it.ownedBy.equals(choice.providerId, ignoreCase = true) }
        .map { it.id }
}

/** Keeps a still-valid selection, otherwise moves to the first id in the new catalog. */
private fun selectProvider(state: ChatUiState, choice: ProviderChoice): ChatUiState {
    val available = availableModelIds(state, choice)
    val nextModel = if (state.selectedModel in available) {
        state.selectedModel
    } else {
        available.firstOrNull().orEmpty()
    }
    return state.copy(selectedProvider = choice, selectedModel = nextModel)
}
