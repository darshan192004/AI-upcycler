package org.aiupcycler.worker.preferences

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map

private val Context.uiPreferencesDataStore: DataStore<Preferences> by preferencesDataStore(
    name = "ui_preferences",
)

class UiPreferencesStore(context: Context) {
    private val dataStore = context.applicationContext.uiPreferencesDataStore

    val flow: Flow<UiPreferences> = dataStore.data.map { prefs ->
        UiPreferences(
            compactMode = prefs[COMPACT_MODE] ?: false,
            showSourceBadges = prefs[SHOW_SOURCE_BADGES] ?: true,
            autoApplyRecommendations = prefs[AUTO_APPLY_RECOMMENDATIONS] ?: false,
        )
    }

    suspend fun setCompactMode(value: Boolean) {
        dataStore.edit { it[COMPACT_MODE] = value }
    }

    suspend fun setShowSourceBadges(value: Boolean) {
        dataStore.edit { it[SHOW_SOURCE_BADGES] = value }
    }

    suspend fun setAutoApplyRecommendations(value: Boolean) {
        dataStore.edit { it[AUTO_APPLY_RECOMMENDATIONS] = value }
    }

    suspend fun writtenKeys(): Set<String> =
        dataStore.data.first().asMap().keys.map { it.name }.toSet()

    companion object {
        private val COMPACT_MODE = booleanPreferencesKey("compact_mode")
        private val SHOW_SOURCE_BADGES = booleanPreferencesKey("show_source_badges")
        private val AUTO_APPLY_RECOMMENDATIONS = booleanPreferencesKey("auto_apply_recommendations")
    }
}
