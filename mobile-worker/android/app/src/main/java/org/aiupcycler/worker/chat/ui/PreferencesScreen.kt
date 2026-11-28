package org.aiupcycler.worker.chat.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.aiupcycler.worker.preferences.UiPreferences
import org.aiupcycler.worker.preferences.UiPreferencesStore

/**
 * Non-secret UI preferences, hosted inside the existing `ChatActivity` tree.
 * Every colour comes from [ChatTheme], which maps one-to-one onto
 * `res/values/colors.xml`; no second theme, host, or navigation graph is
 * introduced here.
 */
@Composable
fun PreferencesScreen(
    store: UiPreferencesStore,
    onClose: () -> Unit,
) {
    ChatTheme {
        val prefs by store.flow.collectAsState(initial = UiPreferences())
        val scope = rememberCoroutineScope()

        Column(
            modifier = Modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.background)
                .padding(16.dp)
                .testTag("preferences-screen"),
        ) {
            Text(
                text = "Preferences",
                style = MaterialTheme.typography.titleLarge,
                color = MaterialTheme.colorScheme.onSurface,
                modifier = Modifier.semantics { contentDescription = "Preferences" },
            )
            PreferenceToggle(
                label = "Compact mode",
                checked = prefs.compactMode,
                onChange = { value -> scope.launch { store.setCompactMode(value) } },
                tag = "preference-compact-mode",
            )
            PreferenceToggle(
                label = "Show source badges",
                checked = prefs.showSourceBadges,
                onChange = { value -> scope.launch { store.setShowSourceBadges(value) } },
                tag = "preference-show-source-badges",
            )
            PreferenceToggle(
                label = "Auto-apply recommended limits",
                checked = prefs.autoApplyRecommendations,
                onChange = { value -> scope.launch { store.setAutoApplyRecommendations(value) } },
                tag = "preference-auto-apply-recommendations",
            )
            Text(
                text = "Close",
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.primary,
                modifier = Modifier
                    .padding(top = 12.dp)
                    .heightIn(min = 48.dp)
                    .clickable(onClick = onClose)
                    .semantics { contentDescription = "Close preferences" }
                    .testTag("preferences-close"),
            )
        }
    }
}

@Composable
private fun PreferenceToggle(
    label: String,
    checked: Boolean,
    onChange: (Boolean) -> Unit,
    tag: String,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .padding(vertical = 4.dp)
            .background(MaterialTheme.colorScheme.surface, RoundedCornerShape(8.dp))
            .testTag(tag),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = label,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurface,
            modifier = Modifier
                .weight(1f)
                .padding(start = 12.dp),
        )
        Switch(
            checked = checked,
            onCheckedChange = onChange,
            modifier = Modifier
                .padding(end = 12.dp)
                .heightIn(min = 48.dp)
                .semantics { contentDescription = "$label toggle" },
        )
    }
}
