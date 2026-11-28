package org.aiupcycler.worker.chat

import android.content.Context
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import java.io.File
import java.io.IOException
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import okhttp3.Call
import org.aiupcycler.worker.chat.secrets.KeystoreSecretStore
import org.aiupcycler.worker.chat.secrets.SecretStore
import org.aiupcycler.worker.pairing.CredentialStore

/**
 * Owns the chat render state and the LAN I/O behind it.
 *
 * Every state transition that the pure reducer models goes through [reduce].
 * The few remaining transitions (optimistic rows, draft edits, busy flag) are
 * direct copies because `ChatEvent` deliberately has no message-mutation event.
 * Credentials are read once for their non-secret fields; the bearer token stays
 * in [SecretStore] and is read by [LanClient] at request time. No value from a
 * secret store is logged or copied into a pref/file.
 */
class ChatViewModel(
    private val application: Context,
    private val credentialDirectory: File = application.filesDir.resolve(PAIRING_DIR),
    private val secrets: SecretStore = KeystoreSecretStore(application),
    private val endpointStorage: LanEndpointStore = LanEndpointStore(SharedPreferencesLanEndpointStorage(
        application.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE),
    )),
    private val callFactory: Call.Factory = LanClient.defaultCallFactory(),
    private val browserFactory: () -> MasterDiscoveryBrowser = { NsdMasterDiscoveryBrowser(application) },
) : ViewModel() {
    private val _uiState = MutableStateFlow(ChatUiState())
    val uiState: StateFlow<ChatUiState> = _uiState

    private val _lanClient = MutableStateFlow<LanClient?>(null)
    val lanClient: StateFlow<LanClient?> = _lanClient

    private val _errorBanner = MutableStateFlow<Banner?>(null)
    private val _pairingRequired = MutableStateFlow(false)

    /** Reducer banners and local load/stream errors in one display surface. */
    val banner: StateFlow<Banner?> = combine(
        _uiState,
        _errorBanner,
        _pairingRequired,
    ) { state, error, needsPairing ->
        when {
            error != null -> error
            needsPairing -> Banner.PairingRequired()
            else -> state.banner
        }
    }.stateIn(viewModelScope, SharingStarted.Eagerly, null)

    private val activeCall = AtomicReference<Call?>(null)
    private val streamActive = AtomicBoolean(false)
    private var discovery: MasterDiscovery? = null
    private var nodeIdValue = ""
    private var fingerprint: String? = null
    private var hasCompletedTurn = false

    /** Guards a session-list read against a newer selection (including A -> B -> A). */
    private val selectionLock = Any()
    private var selectionGeneration = 0L

    /** A stream callback only changes turn state while it still owns the turn. */
    private val turnLock = Any()
    private var turnSequence = 0L
    private var currentTurn: Long? = null

    /** A new session gets a fresh id, so each session is recommended at most once. */
    private val recommendedSessions = ConcurrentHashMap.newKeySet<String>()

    /** Non-secret worker id used by `GET /api/devices/:id/provision`. */
    val nodeId: String get() = nodeIdValue

    init {
        initialize()
    }

    private fun initialize() {
        val credentials = runCatching {
            CredentialStore(credentialDirectory, secrets).load()
        }.getOrNull()

        val tokenPresent = !secrets.read(SecretStore.LAN_TOKEN_KEY).isNullOrBlank()
        if (credentials == null || !tokenPresent || credentials.nodeId.isBlank()) {
            _pairingRequired.value = true
            return
        }

        nodeIdValue = credentials.nodeId
        fingerprint = credentials.fingerprint
        _pairingRequired.value = false

        val stored = endpointStorage.read()
        if (stored != null) {
            useEndpoint(stored)
        } else {
            startDiscovery()
        }
    }

    /**
     * Re-reads credentials after pairing completes on the control screen. The
     * Activity calls this on resume; once a node id and client exist there is
     * nothing to redo. Directory and secret-store wiring stay unchanged.
     */
    fun refreshPairing() {
        if (nodeIdValue.isNotBlank() && _lanClient.value != null) return
        discovery?.stop()
        initialize()
    }

    private fun startDiscovery() {
        val expected = fingerprint
        if (expected.isNullOrBlank()) {
            _pairingRequired.value = true
            return
        }
        discovery = MasterDiscovery(
            browserFactory(),
            { expected },
            { endpoint -> useEndpoint(LanEndpoint(endpoint.host, endpoint.port)) },
            { failure -> _errorBanner.value = Banner.Offline(failure.reason) },
        )
        discovery?.start()
    }

    /** Called from `ChatActivity.onNewIntent` for LAN discovery callback data. */
    fun onDiscoveryEndpoint(endpoint: LanEndpoint) {
        if (secrets.read(SecretStore.LAN_TOKEN_KEY).isNullOrBlank()) {
            _pairingRequired.value = true
            return
        }
        useEndpoint(endpoint)
    }

    private fun useEndpoint(endpoint: LanEndpoint) {
        endpointStorage.save(endpoint)
        discovery?.stop()
        _lanClient.value = LanClient(endpoint, secrets, callFactory)
        _pairingRequired.value = false
        _errorBanner.value = null
        _uiState.update { it.copy(endpoint = endpoint) }
        refreshCatalog()
        refreshSessions()
        refreshMultimodalGate()
        openProvisioning()
    }

    // ---- reducer sink -----------------------------------------------------

    fun onEvent(event: ChatEvent) {
        dispatch(event)
    }

    private fun dispatch(event: ChatEvent) {
        _uiState.update { reduce(it, event) }
    }

    // ---- user actions -----------------------------------------------------

    fun onDraftChange(value: String) {
        _uiState.update { it.copy(draft = value) }
    }

    /**
     * Uploads one picked image through the existing LAN client. The bearer
     * token stays in [SecretStore]; neither the token nor image bytes are
     * logged. Only the returned [MediaRef] is put into composer state.
     */
    fun attachImage(fileName: String, mimeType: String, bytes: ByteArray) {
        val client = _lanClient.value
        if (client == null || !_uiState.value.multimodalEnabled) {
            _errorBanner.value = Banner.InvalidAttachment()
            return
        }
        viewModelScope.launch(Dispatchers.IO) {
            runCatching { client.uploadImage(fileName, mimeType, bytes) }
                .onSuccess { dispatch(ChatEvent.AttachmentAdded(it)) }
                .onFailure { _errorBanner.value = Banner.InvalidAttachment() }
        }
    }

    fun removeAttachment(mediaHash: String) {
        dispatch(ChatEvent.AttachmentRemoved(mediaHash))
    }

    fun newSession() {
        activeCall.getAndSet(null)?.cancel()
        synchronized(turnLock) { currentTurn = null }
        streamActive.set(false)
        hasCompletedTurn = false
        _errorBanner.value = null
        // Advance the selection generation under the same lock as the new id,
        // so a listMessages read for a previous selection cannot fill it.
        synchronized(selectionLock) {
            selectionGeneration += 1
            _uiState.update {
                it.copy(
                    activeSessionId = UUID.randomUUID().toString(),
                    messages = emptyList(),
                    draft = "",
                    attachments = emptyList(),
                    busy = false,
                    source = null,
                    banner = null,
                )
            }
        }
    }

    fun selectSession(sessionId: String) {
        if (sessionId.isBlank()) return
        activeCall.getAndSet(null)?.cancel()
        synchronized(turnLock) { currentTurn = null }
        streamActive.set(false)
        hasCompletedTurn = false
        // Selection and generation advance under one lock, so a read that was
        // already in flight cannot apply to a newer selection.
        val generation = synchronized(selectionLock) {
            selectionGeneration += 1
            _uiState.update { it.copy(activeSessionId = sessionId, messages = emptyList(), busy = false) }
            selectionGeneration
        }

        val client = _lanClient.value ?: return
        viewModelScope.launch(Dispatchers.IO) {
            runCatching { client.listMessages(sessionId) }
                .onSuccess { messages ->
                    synchronized(selectionLock) {
                        if (generation == selectionGeneration) {
                            _uiState.update { it.copy(messages = messages) }
                        }
                    }
                }
                .onFailure { error ->
                    synchronized(selectionLock) {
                        if (generation == selectionGeneration) reportThrowable(error)
                    }
                }
        }
    }

    fun send() {
        val current = _uiState.value
        val text = current.draft.trim()
        if ((text.isBlank() && current.attachments.isEmpty()) || current.busy) return

        val client = _lanClient.value
        if (client == null || nodeIdValue.isBlank()) {
            _pairingRequired.value = true
            return
        }

        val sessionId = current.activeSessionId.ifBlank { UUID.randomUUID().toString() }
        val priorMessages = current.messages.filterNot {
            it.role.equals("assistant", ignoreCase = true) && it.content.isEmpty()
        }
        val userMessage = ChatMessage(role = "user", content = text)
        val assistantMessage = ChatMessage(
            role = "assistant",
            content = "",
            messageId = optimisticId(),
        )
        val request = ChatRequest(
            sessionId = sessionId,
            model = current.selectedModel,
            provider = current.selectedProvider,
            history = (priorMessages + userMessage).map {
                ChatMessage(role = it.role, content = it.content)
            },
            attachments = current.attachments,
        )

        _uiState.update {
            it.copy(
                activeSessionId = sessionId,
                messages = priorMessages + userMessage + assistantMessage,
                draft = "",
                attachments = emptyList(),
                busy = true,
                banner = null,
            )
        }
        _errorBanner.value = null
        val turn = synchronized(turnLock) {
            turnSequence += 1
            currentTurn = turnSequence
            turnSequence
        }
        streamActive.set(true)

        val call = client.streamChat(
            request = request,
            onDelta = { text ->
                synchronized(turnLock) {
                    if (currentTurn == turn) dispatch(ChatEvent.Delta(text))
                }
            },
            onDone = { source -> finishTurn(source, turn) },
            onError = { error -> report(error, fromStream = true, turn = turn) },
        )
        synchronized(turnLock) {
            // A fast [DONE] can already have ended this turn before enqueue returns.
            if (currentTurn == turn) activeCall.set(call)
        }
    }

    fun stop() {
        synchronized(turnLock) { currentTurn = null }
        if (!streamActive.getAndSet(false)) return
        activeCall.getAndSet(null)?.cancel()
        _uiState.update { it.copy(busy = false) }
    }

    fun selectProvider(choice: ProviderChoice) {
        dispatch(ChatEvent.ProviderSelected(choice))
    }

    fun selectModel(modelId: String) {
        dispatch(ChatEvent.ModelSelected(modelId))
    }

    /** Samples the current push state so `ProvisioningProgressScreen` can poll. */
    fun openProvisioning() {
        val client = _lanClient.value ?: return
        if (nodeIdValue.isBlank()) return
        viewModelScope.launch(Dispatchers.IO) {
            runCatching { client.provisionState(nodeIdValue) }
                .onSuccess { dispatch(ChatEvent.ProvisioningUpdated(it)) }
                .onFailure { reportThrowable(it) }
        }
    }

    // ---- LAN refresh ------------------------------------------------------

    private fun refreshCatalog() {
        val client = _lanClient.value ?: return
        viewModelScope.launch(Dispatchers.IO) {
            runCatching { client.listModels() to client.listProviders() }
                .onSuccess { (models, providers) ->
                    dispatch(ChatEvent.ModelCatalogLoaded(models, providers))
                }
                .onFailure { reportThrowable(it) }
        }
    }

    private fun refreshSessions() {
        val client = _lanClient.value ?: return
        viewModelScope.launch(Dispatchers.IO) {
            runCatching { client.listSessions() }
                .onSuccess { dispatch(ChatEvent.SessionListLoaded(it)) }
                .onFailure { reportThrowable(it) }
        }
    }

    /**
     * One recommendation pass per session, used by the auto-apply preference.
     * Order is fixed: get settings, get recommendations, then put settings with
     * only `context_tokens`/`max_tokens` replaced. Provider, model, and UI
     * flags pass through untouched and no secret field is constructed.
     */
    fun applyRecommendationsOnce(sessionId: String) {
        if (sessionId.isBlank()) return
        val client = _lanClient.value ?: return
        if (!recommendedSessions.add(sessionId)) return
        viewModelScope.launch(Dispatchers.IO) {
            runCatching {
                val current = client.getSettings()
                val recommended = client.getSettingsRecommendations()
                client.putSettings(current.withLimitsFrom(recommended))
            }.onFailure { reportThrowable(it) }
        }
    }

    /** The attach control stays hidden until the host proves its gate is on. */
    private fun refreshMultimodalGate() {
        val client = _lanClient.value ?: return
        viewModelScope.launch(Dispatchers.IO) {
            val enabled = runCatching { client.mediaGateEnabled() }.getOrDefault(false)
            dispatch(ChatEvent.MultimodalGateUpdated(enabled))
        }
    }

    private fun finishTurn(source: Source, turn: Long) {
        synchronized(turnLock) {
            if (currentTurn == turn) {
                currentTurn = null
                activeCall.set(null)
                streamActive.set(false)
                dispatch(ChatEvent.StreamDone(source))
                if (!hasCompletedTurn) {
                    hasCompletedTurn = true
                    refreshSessions()
                }
            }
        }
    }

    // ---- named failures ---------------------------------------------------

    private fun report(error: LanError, fromStream: Boolean = false, turn: Long? = null) {
        if (fromStream) {
            val accepted = synchronized(turnLock) {
                if (turn != null && currentTurn == turn) {
                    currentTurn = null
                    activeCall.set(null)
                    streamActive.set(false)
                    true
                } else {
                    false
                }
            }
            if (!accepted) return
        }

        if (error is LanError.Unauthorized) {
            dispatch(ChatEvent.Unauthorized)
        } else {
            _errorBanner.value = when (error) {
                is LanError.Unauthorized -> Banner.Unauthorized()
                LanError.NotFound -> Banner.NotFound()
                LanError.Offline -> Banner.Offline()
                LanError.MalformedResponse -> Banner.MalformedResponse()
                is LanError.Server -> Banner.MalformedResponse("LAN host returned HTTP ${error.code}")
            }
        }
        // A background failure (e.g. provisioning) still shows its banner but
        // must not clear the state of a live stream.
        if (fromStream) _uiState.update { it.copy(busy = false) }
    }

    private fun reportThrowable(throwable: Throwable) {
        val error = when (throwable) {
            is LanException -> throwable.error
            is IOException -> LanError.Offline
            else -> LanError.MalformedResponse
        }
        report(error)
    }

    override fun onCleared() {
        synchronized(turnLock) { currentTurn = null }
        streamActive.set(false)
        activeCall.getAndSet(null)?.cancel()
        discovery?.stop()
        super.onCleared()
    }

    companion object {
        private const val PREFS_NAME = "ai-upcycler-worker"
        private const val PAIRING_DIR = "pairing"

        /** Optimistic rows need a non-null id for `Delta`, never a list index. */
        private fun optimisticId(): Long = UUID.randomUUID().mostSignificantBits or Long.MIN_VALUE

        fun factory(context: Context): ViewModelProvider.Factory = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(modelClass: Class<T>): T {
                if (modelClass.isAssignableFrom(ChatViewModel::class.java)) {
                    return ChatViewModel(context.applicationContext) as T
                }
                throw IllegalArgumentException("Unknown ViewModel class: ${modelClass.name}")
            }
        }
    }
}
