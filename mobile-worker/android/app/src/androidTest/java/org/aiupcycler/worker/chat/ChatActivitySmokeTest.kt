package org.aiupcycler.worker.chat

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * On-device smoke checks for the B launcher. These are intentionally narrow:
 * they prove the overflow is reachable and that it exposes exactly the two
 * documented entries, without depending on a paired host being present.
 */
@RunWith(AndroidJUnit4::class)
class ChatActivitySmokeTest {
    @get:Rule
    val composeTestRule = createAndroidComposeRule<ChatActivity>()

    @Test
    fun overflowMenuHasExactlyTwoActions() {
        composeTestRule.onNodeWithContentDescription("More options")
            .assertIsDisplayed()
            .performClick()

        composeTestRule.onNodeWithText("Control & pairing").assertIsDisplayed()
        composeTestRule.onNodeWithText("About").assertIsDisplayed()
        assertEquals(
            2,
            composeTestRule.onAllNodes(hasTestTag(ChatActivity.OVERFLOW_ITEM_TAG))
                .fetchSemanticsNodes()
                .size,
        )
    }

    @Test
    fun pairingBannerDoesNotHideOverflow() {
        val banner = composeTestRule.onAllNodes(hasTestTag(ChatActivity.PAIRING_BANNER_TAG))
        if (banner.fetchSemanticsNodes().isEmpty()) return

        banner[0].assertIsDisplayed()
        composeTestRule.onNodeWithContentDescription("More options")
            .assertIsDisplayed()
            .performClick()
        composeTestRule.onNodeWithText("Control & pairing").assertIsDisplayed()
    }
}
