package org.aiupcycler.worker.chat.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import org.aiupcycler.worker.chat.ModelEntry
import org.aiupcycler.worker.chat.ProviderChoice
import org.aiupcycler.worker.chat.modelBelongsToProvider

private data class ProviderOption(val choice: ProviderChoice, val label: String)

/** Exactly the six choices accepted by the picker, in display order. */
private val PROVIDER_OPTIONS = listOf(
    ProviderOption(ProviderChoice.Auto, "Auto"),
    ProviderOption(ProviderChoice.Local, "Local"),
    ProviderOption(ProviderChoice.Cloud("openai"), "OpenAI"),
    ProviderOption(ProviderChoice.Cloud("anthropic"), "Anthropic"),
    ProviderOption(ProviderChoice.Cloud("gemini"), "Gemini"),
    ProviderOption(ProviderChoice.Cloud("kimi"), "Kimi"),
)

private fun providerLabel(choice: ProviderChoice): String = when (choice) {
    ProviderChoice.Auto -> "Auto"
    ProviderChoice.Local -> "Local"
    is ProviderChoice.Cloud -> PROVIDER_OPTIONS
        .firstOrNull { it.choice == choice }
        ?.label ?: choice.providerId
}

/**
 * Provider and model choice. [models] is the merged catalog: local `/v1/models`
 * ids stay byte-for-byte, and cloud rows use the virtual `provider:model` id.
 * Every row keeps a 48 dp minimum height; the pressed colour is painted inside
 * the same bounds, so neighbouring rows never move.
 */
@Composable
fun ProviderPicker(
    models: List<ModelEntry>,
    selectedProvider: ProviderChoice,
    selectedModel: String,
    onProviderSelected: (ProviderChoice) -> Unit,
    onModelSelected: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    var providerExpanded by remember { mutableStateOf(false) }
    var modelExpanded by remember { mutableStateOf(false) }

    Row(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(modifier = Modifier.weight(1f)) {
            val providerTrigger = "Provider: ${providerLabel(selectedProvider)}"
            PressableRow(
                label = providerTrigger,
                selected = false,
                mono = false,
                onClick = { providerExpanded = true },
            )
            DropdownMenu(
                expanded = providerExpanded,
                onDismissRequest = { providerExpanded = false },
            ) {
                PROVIDER_OPTIONS.forEach { option ->
                    PressableRow(
                        label = option.label,
                        selected = option.choice == selectedProvider,
                        mono = false,
                        onClick = {
                            onProviderSelected(option.choice)
                            providerExpanded = false
                        },
                    )
                }
            }
        }

        Spacer(Modifier.width(8.dp))

        Column(modifier = Modifier.weight(1f)) {
            val modelTrigger = if (selectedModel.isBlank()) "Model" else selectedModel
            PressableRow(
                label = modelTrigger,
                selected = false,
                mono = true,
                onClick = { modelExpanded = true },
            )
            DropdownMenu(
                expanded = modelExpanded,
                onDismissRequest = { modelExpanded = false },
            ) {
                models.filter { modelBelongsToProvider(it, selectedProvider) }.forEach { model ->
                    PressableRow(
                        label = model.id,
                        selected = model.id == selectedModel,
                        mono = true,
                        onClick = {
                            onModelSelected(model.id)
                            modelExpanded = false
                        },
                    )
                }
            }
        }
    }
}

/**
 * A dropdown row with a fixed height and a paint-only pressed state. The icon
 * is decorative; [label] is the row's accessible name.
 */
@Composable
private fun PressableRow(
    label: String,
    selected: Boolean,
    mono: Boolean,
    onClick: () -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val background = when {
        pressed || selected -> MaterialTheme.colorScheme.surfaceVariant
        else -> MaterialTheme.colorScheme.surface
    }

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .background(background)
            .clickable(
                interactionSource = interaction,
                indication = null,
                role = Role.DropdownList,
                onClick = onClick,
            )
            .semantics { contentDescription = label }
            .padding(horizontal = 8.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PickerGlyph()
        Spacer(Modifier.width(8.dp))
        Text(
            text = label,
            modifier = Modifier.weight(1f),
            style = MaterialTheme.typography.labelMedium.copy(
                fontFamily = if (mono) FiraCode else FiraSans,
            ),
            color = MaterialTheme.colorScheme.onSurface,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** Decorative outline glyph; text beside it carries the meaning. */
@Composable
private fun PickerGlyph() {
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
