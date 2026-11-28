package org.aiupcycler.worker.chat.ui

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.interaction.collectIsHoveredAsState
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import org.aiupcycler.worker.R
import org.aiupcycler.worker.chat.ChatMessage
import org.aiupcycler.worker.chat.MediaRef

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
    attachments: List<MediaRef> = emptyList(),
    multimodalEnabled: Boolean = false,
    onAttachImage: (fileName: String, mimeType: String, bytes: ByteArray) -> Unit = { _, _, _ -> },
    onRemoveAttachment: (String) -> Unit = {},
    compact: Boolean = false,
    modifier: Modifier = Modifier,
) {
    val canSend = (draft.isNotBlank() || attachments.isNotEmpty()) && !busy
    val context = LocalContext.current
    val imagePicker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        val mime = context.contentResolver.getType(uri) ?: "image/png"
        val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() }
        if (bytes != null) onAttachImage(displayName(context, uri), mime, bytes)
    }
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
                MessageRow(message = message, busy = busy, compact = compact)
            }
            if (busy && !hasAssistantRow) {
                item(key = "assistant-placeholder") {
                    GeneratingSkeleton()
                }
            }
        }

        if (attachments.isNotEmpty()) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .horizontalScroll(rememberScrollState())
                    .padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                attachments.forEach { ref ->
                    AttachmentChip(
                        data = ref,
                        onRemove = { onRemoveAttachment(ref.mediaHash) },
                    )
                    Spacer(Modifier.width(8.dp))
                }
            }
        }

        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 8.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (multimodalEnabled) {
                Button(
                    onClick = { imagePicker.launch("image/*") },
                    modifier = Modifier
                        .heightIn(min = 48.dp)
                        .semantics { contentDescription = "Attach image" },
                    shape = RoundedCornerShape(8.dp),
                ) {
                    Text(text = "Attach")
                }
                Spacer(Modifier.width(8.dp))
            }
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

/**
 * One image attachment. The file name is its own text node so accessibility
 * assertions and screen readers can name it exactly; MIME and byte count share
 * a second caption. Focus is the accent token; destructive hover is tokenised
 * too, and reduced motion keeps the chip static.
 */
@Composable
fun AttachmentChip(data: MediaRef, onRemove: () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val hovered by interaction.collectIsHoveredAsState()
    val reducedMotion = LocalChatReducedMotion.current
    val focusColor = MaterialTheme.colorScheme.primary
    val destructive = MaterialTheme.colorScheme.error

    Row(
        modifier = Modifier
            .heightIn(min = 48.dp)
            .background(MaterialTheme.colorScheme.surface, RoundedCornerShape(8.dp))
            .border(1.dp, MaterialTheme.colorScheme.outline, RoundedCornerShape(8.dp))
            .padding(start = 8.dp, end = 4.dp)
            .semantics { contentDescription = "image attachment, ${data.fileName}, remove button" },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = data.fileName,
            style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FiraSans),
            color = MaterialTheme.colorScheme.onSurface,
        )
        Spacer(Modifier.width(8.dp))
        Text(
            text = "${data.mimeType} · ${data.byteCount} B",
            style = MaterialTheme.typography.labelSmall.copy(fontFamily = FiraSans),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(Modifier.width(8.dp))
        IconButton(
            onClick = onRemove,
            interactionSource = interaction,
            modifier = Modifier
                .heightIn(min = 48.dp)
                .background(
                    color = if (hovered && !reducedMotion) {
                        destructive.copy(alpha = 0.16f)
                    } else {
                        Color.Transparent
                    },
                    shape = RoundedCornerShape(8.dp),
                )
                .border(
                    width = if (focused) 2.dp else 0.dp,
                    color = focusColor,
                    shape = RoundedCornerShape(8.dp),
                )
                .semantics { contentDescription = "Remove ${data.fileName}" },
        ) {
            Text(
                text = "×",
                style = MaterialTheme.typography.titleMedium,
                color = destructive,
            )
        }
    }
}

private fun displayName(context: Context, uri: Uri): String {
    val cursor = context.contentResolver.query(
        uri,
        arrayOf(OpenableColumns.DISPLAY_NAME),
        null,
        null,
        null,
    )
    return cursor?.use { rows ->
        val column = rows.getColumnIndex(OpenableColumns.DISPLAY_NAME)
        if (column >= 0 && rows.moveToFirst()) rows.getString(column) else null
    }?.takeIf { it.isNotBlank() } ?: "image"
}

@Composable
private fun MessageRow(
    message: ChatMessage,
    busy: Boolean,
    compact: Boolean,
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
                    modifier = Modifier.padding(
                        horizontal = 8.dp,
                        vertical = if (compact) 4.dp else 8.dp,
                    ),
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
