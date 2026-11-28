package org.aiupcycler.worker.chat.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import org.aiupcycler.worker.chat.ChatEvent
import org.aiupcycler.worker.chat.ChatUiState

/**
 * Chat composition root. Task 12 wires this to `ChatActivity` with state from
 * its view model; Task 11 adds provider/source/progress content here.
 *
 * [onEvent] is the reducer-event sink (SessionListLoaded, Delta, StreamDone,
 * ...); the action callbacks map directly to the view model's send/stop and
 * session methods.
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
    modifier: Modifier = Modifier,
) {
    ChatTheme {
        Column(
            modifier = modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.background),
            verticalArrangement = Arrangement.Top,
            horizontalAlignment = Alignment.Start,
        ) {
            SessionListScreen(
                sessions = state.sessions,
                activeSessionId = state.activeSessionId,
                onNewSession = onNewSession,
                onSelectSession = onSelectSession,
                modifier = Modifier.weight(1f),
            )
            ChatThreadScreen(
                messages = state.messages,
                draft = state.draft,
                busy = state.busy,
                onDraftChange = onDraftChange,
                onSend = onSend,
                onStop = onStop,
                modifier = Modifier.weight(1f),
            )
        }
    }
}
