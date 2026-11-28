package org.aiupcycler.worker.chat

import androidx.compose.ui.test.assertHasClickAction
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.aiupcycler.worker.chat.ui.AttachmentChip
import org.aiupcycler.worker.chat.ui.ChatScreen
import org.aiupcycler.worker.chat.ui.ChatTheme
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

    /** Task 11 RED assertions: the chip must expose a separate file-name text node. */
    @Test
    fun chipShowsFileNameAndRemoveLabel() {
        composeTestRule.setContent {
            ChatTheme {
                AttachmentChip(
                    data = MediaRef(
                        mediaHash = "a".repeat(64),
                        mimeType = "image/png",
                        fileName = "a.png",
                        byteCount = 9L,
                    ),
                    onRemove = {},
                )
            }
        }

        val composeRule = composeTestRule
        composeRule.onNodeWithText("a.png").assertIsDisplayed()
        composeRule.onNodeWithContentDescription("Remove a.png").assertIsDisplayed()
        composeRule.onNodeWithContentDescription("Remove a.png").assertHasClickAction()
    }

    /** Task 11 RED assertions: the gate defaults to off, so no attach control exists. */
    @Test
    fun attachButtonHiddenWhenGateOff() {
        composeTestRule.setContent {
            ChatScreen(
                state = ChatUiState(multimodalEnabled = false),
                onEvent = {},
                onNewSession = {},
            )
        }

        val composeRule = composeTestRule
        composeRule.onNodeWithContentDescription("Attach image").assertDoesNotExist()
    }
}
