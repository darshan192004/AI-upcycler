package org.aiupcycler.worker.chat.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import org.aiupcycler.worker.R
import org.aiupcycler.worker.chat.ChatMessage

/**
 * Streaming thread. Every row is keyed by [ChatMessage.stableId], so a delta
 * updates the existing assistant bubble in place instead of re-keying a list
 * index. Text is rendered as it arrives; generation never shows a spinner.
 */
@Composable
fun ChatThreadScreen(
    messages: List<ChatMessage>,
    draft: String,
    busy: Boolean,
    onDraftChange: (String) -> Unit,
    onSend: () -> Unit,
    onStop: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val canSend = draft.isNotBlank() && !busy
    // The skeleton belongs only to the placeholder before the first delta.
    // Once an assistant row exists it owns its own skeleton (empty content) or
    // renders streamed text directly, so no second skeleton is ever added.
    val hasAssistantRow = messages.any { it.role.equals("assistant", ignoreCase = true) }

    Column(modifier = modifier.fillMaxSize()) {
        LazyColumn(
            modifier = Modifier
                .fillMaxWidth()
                .weight(1f),
            contentPadding = PaddingValues(horizontal = 8.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(items = messages, key = { it.stableId }) { message ->
                MessageRow(message = message, busy = busy)
            }
            if (busy && !hasAssistantRow) {
                item(key = "assistant-placeholder") {
                    GeneratingSkeleton()
                }
            }
        }

        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 8.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            OutlinedTextField(
                value = draft,
                onValueChange = onDraftChange,
                modifier = Modifier
                    .weight(1f)
                    .heightIn(min = 48.dp),
                placeholder = { Text(text = stringResource(R.string.chat_draft_hint)) },
                textStyle = LocalTextStyle.current.copy(fontFamily = FiraSans),
            )
            Spacer(Modifier.width(8.dp))
            Button(
                onClick = onSend,
                enabled = canSend,
                modifier = Modifier
                    .heightIn(min = 48.dp)
                    .alpha(if (canSend) 1f else 0.6f),
                shape = RoundedCornerShape(8.dp),
                colors = ButtonDefaults.buttonColors(
                    disabledContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                    disabledContentColor = MaterialTheme.colorScheme.onSurfaceVariant,
                ),
            ) {
                Text(
                    text = stringResource(R.string.chat_send),
                    style = MaterialTheme.typography.labelLarge.copy(fontFamily = FiraSansMedium),
                )
            }
            if (busy) {
                Spacer(Modifier.width(8.dp))
                Button(
                    onClick = onStop,
                    modifier = Modifier.heightIn(min = 48.dp),
                    shape = RoundedCornerShape(8.dp),
                    colors = ButtonDefaults.buttonColors(
                        containerColor = MaterialTheme.colorScheme.error,
                        contentColor = MaterialTheme.colorScheme.background,
                    ),
                ) {
                    Text(
                        text = stringResource(R.string.chat_stop_generation),
                        style = MaterialTheme.typography.labelLarge.copy(fontFamily = FiraSansMedium),
                    )
                }
            }
        }
    }
}

@Composable
private fun MessageRow(
    message: ChatMessage,
    busy: Boolean,
) {
    val isUser = message.role.equals("user", ignoreCase = true)
    val label = stringResource(if (isUser) R.string.chat_role_you else R.string.chat_role_assistant)
    val showSkeleton = !isUser && busy && message.content.isEmpty()
    val shape = if (isUser) {
        RoundedCornerShape(
            topStart = 12.dp,
            topEnd = 0.dp,
            bottomStart = 12.dp,
            bottomEnd = 12.dp,
        )
    } else {
        RoundedCornerShape(
            topStart = 0.dp,
            topEnd = 12.dp,
            bottomStart = 12.dp,
            bottomEnd = 12.dp,
        )
    }

    Column(
        modifier = Modifier.fillMaxWidth(),
        horizontalAlignment = if (isUser) Alignment.End else Alignment.Start,
    ) {
        Text(
            text = label,
            modifier = Modifier.semantics { contentDescription = label },
            style = MaterialTheme.typography.labelSmall.copy(fontFamily = FiraSansMedium),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.height(4.dp))
        if (showSkeleton) {
            GeneratingSkeleton()
        } else {
            Surface(
                shape = shape,
                color = if (isUser) MaterialTheme.colorScheme.surfaceVariant else MaterialTheme.colorScheme.surface,
                border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
            ) {
                Text(
                    text = message.content,
                    modifier = Modifier.padding(horizontal = 8.dp, vertical = 8.dp),
                    style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FiraSans),
                    color = MaterialTheme.colorScheme.onSurface,
                )
            }
        }
    }
}

/**
 * Static two-bar skeleton for the brief moment before the first token. No
 * animation: reduced-motion is respected by construction, and the key is the
 * owning message's [ChatMessage.stableId] so it never flickers on a delta.
 */
@Composable
private fun GeneratingSkeleton() {
    val barAlpha = if (LocalChatReducedMotion.current) 0.55f else 0.8f
    val barColor = MaterialTheme.colorScheme.outline

    Column(
        modifier = Modifier
            .heightIn(min = 48.dp)
            .padding(8.dp)
            .semantics { stateDescription = "Generating" },
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(
            text = stringResource(R.string.chat_generating),
            style = MaterialTheme.typography.labelSmall.copy(fontFamily = FiraSans),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Box(
            modifier = Modifier
                .fillMaxWidth(0.8f)
                .height(8.dp)
                .alpha(barAlpha)
                .background(barColor, RoundedCornerShape(4.dp)),
        )
        Box(
            modifier = Modifier
                .fillMaxWidth(0.5f)
                .height(8.dp)
                .alpha(barAlpha)
                .background(barColor, RoundedCornerShape(4.dp)),
        )
    }
}
