package org.aiupcycler.worker.chat.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.dp
import org.aiupcycler.worker.chat.ProviderStatus
import org.aiupcycler.worker.chat.Source
import org.aiupcycler.worker.chat.sourceBadgeText

private data class BadgeTokens(
    val background: Color,
    val border: Color,
    val content: Color,
)

/** Where a completed turn actually ran, derived from the wire source only. */
@Composable
fun SourceBadge(
    source: Source?,
    providers: List<ProviderStatus> = emptyList(),
    modifier: Modifier = Modifier,
) {
    val text = sourceBadgeText(source, providers)
    val tokens = badgeTokens(source)

    Surface(
        modifier = modifier.heightIn(min = 48.dp),
        shape = RoundedCornerShape(8.dp),
        color = tokens.background,
        border = BorderStroke(1.dp, tokens.border),
    ) {
        Row(
            modifier = Modifier.padding(horizontal = 8.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            SourceGlyph(tokens.content)
            Spacer(Modifier.width(8.dp))
            Text(
                text = text,
                style = MaterialTheme.typography.labelMedium.copy(fontFamily = FiraSansMedium),
                color = tokens.content,
            )
        }
    }
}

/** All colours come from the theme, which maps directly to `colors.xml`. */
@Composable
private fun badgeTokens(source: Source?): BadgeTokens = when (source) {
    Source.Local -> BadgeTokens(
        background = MaterialTheme.colorScheme.surface,
        border = MaterialTheme.colorScheme.primary,
        content = MaterialTheme.colorScheme.primary,
    )
    is Source.Worker -> BadgeTokens(
        background = MaterialTheme.colorScheme.background,
        border = MaterialTheme.colorScheme.outline,
        content = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    is Source.Provider -> BadgeTokens(
        background = MaterialTheme.colorScheme.surface,
        border = MaterialTheme.colorScheme.outline,
        content = MaterialTheme.colorScheme.onSurface,
    )
    null -> BadgeTokens(
        background = MaterialTheme.colorScheme.surfaceVariant,
        border = MaterialTheme.colorScheme.outline,
        content = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

/** Decorative glyph; the adjacent text is the badge's accessible name. */
@Composable
private fun SourceGlyph(content: Color) {
    Canvas(
        modifier = Modifier
            .size(20.dp)
            .clearAndSetSemantics {},
    ) {
        drawRoundRect(
            color = content,
            cornerRadius = CornerRadius(4.dp.toPx()),
            style = Stroke(width = 1.5.dp.toPx()),
        )
        drawCircle(
            color = content,
            radius = 3.dp.toPx(),
            center = Offset(size.width / 2f, size.height / 2f),
        )
    }
}
