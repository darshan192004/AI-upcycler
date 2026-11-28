package org.aiupcycler.worker.preferences

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.ConscryptMode

// The app ships org.conscrypt:conscrypt-android for gRPC ALPN on device. On the
// JVM Robolectric would try to load its native library during application setup
// and fail with UnsatisfiedLinkError. BouncyCastle covers these preference tests.
@RunWith(RobolectricTestRunner::class)
@ConscryptMode(ConscryptMode.Mode.OFF)
class UiPreferencesStoreTest {
    private val context: Context = ApplicationProvider.getApplicationContext()

    @Test
    fun defaults_are_stable() = runTest {
        val store = UiPreferencesStore(context)

        assertEquals(UiPreferences(), store.flow.first())
    }

    @Test
    fun writes_only_ui_preference_keys() = runTest {
        val store = UiPreferencesStore(context)

        store.setCompactMode(true)
        store.setShowSourceBadges(false)
        store.setAutoApplyRecommendations(true)

        assertEquals(
            UiPreferences(
                compactMode = true,
                showSourceBadges = false,
                autoApplyRecommendations = true,
            ),
            store.flow.first(),
        )
        assertEquals(
            setOf("compact_mode", "show_source_badges", "auto_apply_recommendations"),
            store.writtenKeys(),
        )
    }
}
