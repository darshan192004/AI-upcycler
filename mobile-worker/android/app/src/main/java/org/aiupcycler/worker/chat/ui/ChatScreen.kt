package org.aiupcycler.worker.chat.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.flowOf
import org.aiupcycler.worker.chat.ChatEvent
import org.aiupcycler.worker.chat.ChatUiState
import org.aiupcycler.worker.chat.LanClient
import org.aiupcycler.worker.chat.ProviderChoice
import org.aiupcycler.worker.chat.ProvisionState
import org.aiupcycler.worker.preferences.UiPreferences
import org.aiupcycler.worker.preferences.UiPreferencesStore
import org.aiupcycler.worker.images.ImageApi
import org.aiupcycler.workflow.LanWorkflowApi
import org.aiupcycler.workflow.WorkflowApi

/**
 * Chat composition root. Task 12 wires this to `ChatActivity` with state from
 * its view model; Task 11 adds provider/source/progress content here.
 *
 * [onEvent] is the reducer-event sink (SessionListLoaded, Delta, StreamDone,
 * ProvisioningUpdated, ...); the action callbacks map directly to the view
 * model's send/stop, session, provider, and model methods. The optional
 * [provisioningClient] and [nodeId] enable polling when Task 12 wires the
 * view model; without them the screen still renders the reducer's progress.
 *
 * [preferencesStore] is constructed once by the host and shared with the
 * Preferences surface. Its non-secret flags hide the source badge, tighten
 * thread spacing, and (once per new session) apply host recommendations via
 * [onApplyRecommendations]. Chat history stays on the B LAN client.
 *
 * [imageApi] is the single image client built by the host from B's LAN
 * endpoint and token provider. It backs the one `Images` slot below; when it
 * is absent the slot explains that pairing is required.
 */
@Composable
fun ChatScreen(
    state: ChatUiState,
    onEvent: (ChatEvent) -> Unit,
    onNewSession: () -> Unit,
    onSelectSession: (String) -> Unit = {},
    onDraftChange: (String) -> Unit = {},
    onSend: () -> Unit = {},
    onStop: () -> Unit = {},
    onAttachImage: (fileName: String, mimeType: String, bytes: ByteArray) -> Unit = { _, _, _ -> },
    onRemoveAttachment: (String) -> Unit = {},
    onProviderSelected: (ProviderChoice) -> Unit = {},
    onModelSelected: (String) -> Unit = {},
    provisioningClient: LanClient? = null,
    nodeId: String = "",
    modifier: Modifier = Modifier,
    workflowApi: WorkflowApi? = null,
    preferencesStore: UiPreferencesStore? = null,
    onOpenPreferences: () -> Unit = {},
    onApplyRecommendations: (String) -> Unit = {},
    imageApi: ImageApi? = null,
) {
    val derivedWorkflowApi = remember(provisioningClient) {
        provisioningClient?.let { LanWorkflowApi(it) }
    }
    val api = workflowApi ?: derivedWorkflowApi
    val prefsFlow = remember(preferencesStore) {
        preferencesStore?.flow ?: flowOf(UiPreferences())
    }
    val prefs by prefsFlow.collectAsState(initial = UiPreferences())
    val renderedState = if (prefs.compactMode || state.compact) {
        state.copy(compact = true)
    } else {
        state
    }

    LaunchedEffect(prefs.autoApplyRecommendations, renderedState.activeSessionId, provisioningClient) {
        if (!prefs.autoApplyRecommendations) return@LaunchedEffect
        val sessionId = renderedState.activeSessionId
        if (sessionId.isNotBlank() && provisioningClient != null) {
            onApplyRecommendations(sessionId)
        }
    }

    ChatTheme {
        var selectedTab by remember { mutableStateOf(ChatTab.Chat) }
        Column(
            modifier = modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.background),
            verticalArrangement = Arrangement.Top,
            horizontalAlignment = Alignment.Start,
        ) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = 48.dp)
                    .padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Button(
                    onClick = { selectedTab = ChatTab.Chat },
                    modifier = Modifier
                        .heightIn(min = 48.dp)
                        .testTag("chat-tab"),
                ) {
                    Text(text = "Chat")
                }
                Button(
                    onClick = { selectedTab = ChatTab.Tasks },
                    modifier = Modifier
                        .heightIn(min = 48.dp)
                        .testTag("tasks-tab"),
                ) {
                    Text(text = "Tasks")
                }
                Button(
                    onClick = { selectedTab = ChatTab.Images },
                    modifier = Modifier
                        .heightIn(min = 48.dp)
                        .testTag("images-tab"),
                ) {
                    Text(text = "Images")
                }
                Button(
                    onClick = onOpenPreferences,
                    modifier = Modifier
                        .heightIn(min = 48.dp)
                        .testTag("preferences-tab"),
                ) {
                    Text(text = "Preferences")
                }
            }
            if (api != null && selectedTab == ChatTab.Tasks) {
                org.aiupcycler.workflow.WorkflowProgressScreen(
                    api = api,
                    modifier = Modifier.weight(1f),
                )
            } else if (selectedTab == ChatTab.Images) {
                if (imageApi != null) {
                    Box(modifier = Modifier.weight(1f)) {
                        imageGalleryScreen(
                            api = imageApi,
                            onClose = { selectedTab = ChatTab.Chat },
                        )
                    }
                } else {
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .weight(1f)
                            .padding(16.dp),
                    ) {
                        Text(
                            text = "Pair with a host to view generated images",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurface,
                            modifier = Modifier.testTag("image-gallery-unavailable"),
                        )
                    }
                }
            } else {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(min = 48.dp)
                        .padding(horizontal = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    ProviderPicker(
                        models = renderedState.models,
                        selectedProvider = renderedState.selectedProvider,
                        selectedModel = renderedState.selectedModel,
                        onProviderSelected = onProviderSelected,
                        onModelSelected = onModelSelected,
                        modifier = Modifier.weight(1f),
                    )
                    if (prefs.showSourceBadges) {
                        SourceBadge(
                            source = renderedState.source,
                            providers = renderedState.providers,
                        )
                    }
                }
                if (renderedState.provisioning !is ProvisionState.Idle) {
                    ProvisioningProgressScreen(
                        client = provisioningClient,
                        nodeId = nodeId,
                        provisioning = renderedState.provisioning,
                        onProvisioningChange = { onEvent(ChatEvent.ProvisioningUpdated(it)) },
                    )
                }
                SessionListScreen(
                    sessions = renderedState.sessions,
                    activeSessionId = renderedState.activeSessionId,
                    onNewSession = onNewSession,
                    onSelectSession = onSelectSession,
                    modifier = Modifier.weight(1f),
                )
                ChatThreadScreen(
                    messages = renderedState.messages,
                    draft = renderedState.draft,
                    busy = renderedState.busy,
                    attachments = renderedState.attachments,
                    multimodalEnabled = renderedState.multimodalEnabled,
                    onDraftChange = onDraftChange,
                    onSend = onSend,
                    onStop = onStop,
                    onAttachImage = onAttachImage,
                    onRemoveAttachment = onRemoveAttachment,
                    compact = renderedState.compact,
                    modifier = Modifier.weight(1f),
                )
            }
        }
    }
}

/** The one host-owned tab row: chat, workflows, generated images, preferences. */
private enum class ChatTab { Chat, Tasks, Images }
