package org.aiupcycler.worker.chat.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import org.aiupcycler.worker.R
import org.aiupcycler.worker.chat.ChatSession

/**
 * Session list. Identity is the exact `session_id` string, never a list index:
 * a row is keyed by [ChatSession.sessionId] so reordering cannot re-compose it.
 */
@Composable
fun SessionListScreen(
    sessions: List<ChatSession>,
    activeSessionId: String,
    onNewSession: () -> Unit,
    onSelectSession: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    LazyColumn(
        modifier = modifier.fillMaxSize(),
        contentPadding = WindowInsets.statusBars
            .only(WindowInsetsSides.Top)
            .asPaddingValues(),
    ) {
        item(key = "new-session") {
            // Fixed height and no state-driven padding: pressing only paints a
            // ripple, so neighbouring rows never move.
            Button(
                onClick = onNewSession,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 8.dp, vertical = 8.dp)
                    .heightIn(min = 48.dp),
                shape = RoundedCornerShape(8.dp),
                colors = ButtonDefaults.buttonColors(
                    containerColor = MaterialTheme.colorScheme.primary,
                    contentColor = MaterialTheme.colorScheme.background,
                ),
            ) {
                Text(
                    text = stringResource(R.string.chat_new_session),
                    style = MaterialTheme.typography.labelLarge.copy(fontFamily = FiraSansMedium),
                )
            }
        }

        if (sessions.isEmpty()) {
            item(key = "no-sessions") {
                Text(
                    text = stringResource(R.string.chat_no_sessions),
                    modifier = Modifier.padding(horizontal = 8.dp, vertical = 8.dp),
                    style = MaterialTheme.typography.bodyMedium.copy(fontFamily = FiraSans),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        } else {
            items(items = sessions, key = { it.sessionId }) { session ->
                SessionRow(
                    session = session,
                    active = session.sessionId == activeSessionId,
                    onClick = { onSelectSession(session.sessionId) },
                )
            }
        }
    }
}

@Composable
private fun SessionRow(
    session: ChatSession,
    active: Boolean,
    onClick: () -> Unit,
) {
    val openLabel = stringResource(R.string.chat_open_session)
    val rowBackground = if (active) MaterialTheme.colorScheme.surfaceVariant else MaterialTheme.colorScheme.background

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .background(rowBackground)
            .clickable(onClickLabel = openLabel, onClick = onClick)
            .semantics { selected = active }
            .padding(horizontal = 8.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // Decorative provider-local glyph; the text beside it is the name.
        ProviderLocalIcon()
        Spacer(Modifier.width(8.dp))
        Column(modifier = Modifier.weight(1f)) {
            Text(
                text = session.title,
                style = MaterialTheme.typography.bodyLarge.copy(fontFamily = FiraSans),
                color = MaterialTheme.colorScheme.onSurface,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                text = session.modelId,
                style = MaterialTheme.typography.bodySmall.copy(fontFamily = FiraCode),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Spacer(Modifier.width(8.dp))
        Text(
            text = stringResource(R.string.chat_provider_local),
            style = MaterialTheme.typography.labelMedium.copy(fontFamily = FiraSans),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun ProviderLocalIcon() {
    val outline = MaterialTheme.colorScheme.outline
    val accent = MaterialTheme.colorScheme.primary
    Canvas(
        modifier = Modifier
            .size(20.dp)
            .clearAndSetSemantics {},
    ) {
        drawRoundRect(
            color = outline,
            cornerRadius = CornerRadius(4.dp.toPx()),
            style = Stroke(width = 1.5.dp.toPx()),
        )
        drawCircle(
            color = accent,
            radius = 3.dp.toPx(),
            center = Offset(size.width / 2f, size.height / 2f),
        )
    }
}
