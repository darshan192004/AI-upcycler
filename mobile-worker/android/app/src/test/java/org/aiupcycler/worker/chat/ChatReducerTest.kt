package org.aiupcycler.worker.chat

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Device-free contract for the task 9 chat reducer.
 *
 * Every value here is synthetic: no real bearer token, provider key, serial,
 * or host address is used. The reducer is exercised as a pure function.
 */
class ChatReducerTest {

    private val baseState = ChatUiState(
        endpoint = LanEndpoint.parse("192.168.1.20:8090"),
        activeSessionId = "s-001",
        draft = "draft text",
        busy = true,
        reducedMotion = true,
        compact = true,
    )

    @Test
    fun sessionListLoadedKeepsSessionIdStringsAsKeys() {
        val sessions = listOf(
            ChatSession(sessionId = "007", title = "Numeric-looking", modelId = "local-a", createdAt = "t1"),
            ChatSession(sessionId = "s/with:chars", title = "Punctuated", modelId = "local-b", createdAt = "t2"),
            ChatSession(sessionId = "Case-Sensitive", title = "Mixed case", modelId = "local-c", createdAt = "t3"),
        )

        val next = reduce(baseState, ChatEvent.SessionListLoaded(sessions))

        assertEquals(sessions.map { it.sessionId }, next.sessions.map { it.sessionId })
        assertEquals(sessions, next.sessions)
        assertNotSame(sessions, next.sessions)
    }

    @Test
    fun sessionListLoadedAddingSessionNeverRekeysExistingEntries() {
        val first = ChatSession(sessionId = "s-001", title = "First", modelId = "local-a", createdAt = "t1")
        val second = ChatSession(sessionId = "s-002", title = "Second", modelId = "local-b", createdAt = "t2")
        val added = ChatSession(sessionId = "s-003", title = "Added", modelId = "local-c", createdAt = "t3")

        val afterFirst = reduce(baseState, ChatEvent.SessionListLoaded(listOf(first, second)))
        val afterAdd = reduce(afterFirst, ChatEvent.SessionListLoaded(listOf(first, second, added)))

        assertEquals(listOf("s-001", "s-002", "s-003"), afterAdd.sessions.map { it.sessionId })
        assertSame(first, afterAdd.sessions[0])
        assertSame(second, afterAdd.sessions[1])
        assertSame(added, afterAdd.sessions[2])
        assertEquals(first, afterFirst.sessions[0])
    }

    @Test
    fun deltaAppendsToAssistantMessageByIdNeverLastIndex() {
        val olderAssistant = ChatMessage(role = "assistant", content = "old", messageId = 7L, sessionId = "s-001")
        val streamingAssistant = ChatMessage(role = "assistant", content = "He", messageId = 42L, sessionId = "s-001")
        val trailingNote = ChatMessage(role = "system", content = "tail", messageId = 99L, sessionId = "s-001")
        val state = baseState.copy(messages = listOf(olderAssistant, streamingAssistant, trailingNote))

        val once = reduce(state, ChatEvent.Delta("llo"))
        val twice = reduce(once, ChatEvent.Delta(" world"))

        assertEquals("He", streamingAssistant.content)
        assertEquals("Hello world", twice.messages.first { it.messageId == 42L }.content)
        assertEquals("old", twice.messages.first { it.messageId == 7L }.content)
        assertEquals(trailingNote, twice.messages.last())
        assertSame(trailingNote, twice.messages.last())
        assertEquals(listOf(7L, 42L, 99L), twice.messages.map { it.messageId })
        assertNotSame(state.messages, twice.messages)
    }

    @Test
    fun streamDoneSetsSourceAndClearsBusy() {
        val source = Source.Worker("adb:worker-7")

        val next = reduce(baseState.copy(busy = true, source = null), ChatEvent.StreamDone(source))

        assertEquals(source, next.source)
        assertFalse(next.busy)
        assertEquals(baseState.draft, next.draft)
    }

    @Test
    fun provisioningUpdatedStoresRunningBytes() {
        val running = ProvisionState.Running(bytesDone = 12_345L, totalBytes = 67_890L)

        val next = reduce(baseState, ChatEvent.ProvisioningUpdated(running))

        assertEquals(running, next.provisioning)
        assertEquals(12_345L, (next.provisioning as ProvisionState.Running).bytesDone)
        assertEquals(67_890L, (next.provisioning as ProvisionState.Running).totalBytes)
    }

    @Test
    fun provisioningUpdatedStoresExactFailureMessage() {
        val message = "provision failed: HTTP 503 (retry later)"

        val next = reduce(baseState, ChatEvent.ProvisioningUpdated(ProvisionState.Failed(message)))

        assertEquals(ProvisionState.Failed(message), next.provisioning)
        assertEquals(message, (next.provisioning as ProvisionState.Failed).message)
    }

    @Test
    fun unauthorizedSetsBlockingBannerWithExactUserMessage() {
        val next = reduce(baseState, ChatEvent.Unauthorized)
        val banner = next.banner

        assertTrue(banner is Banner.Unauthorized)
        assertEquals("LAN credentials rejected; pair again", banner?.message)
        assertTrue(banner!!.blocking)
        assertNull(baseState.banner)
    }

    @Test
    fun modelCatalogLoadedKeepsLocalIdsAndAddsSortedProviderIdsOnce() {
        val localZ = ModelEntry(id = "z-local")
        val localAlreadyPrefixed = ModelEntry(id = "beta:dup")
        val localA = ModelEntry(id = "qwen-local")
        val models = listOf(localZ, localAlreadyPrefixed, localA)
        val providers = listOf(
            ProviderStatus(
                id = "zeta",
                name = "Zeta",
                configured = true,
                models = listOf(ProviderModelWire(id = "m2", label = "M2"), ProviderModelWire(id = "m1", label = "M1")),
            ),
            ProviderStatus(
                id = "alpha",
                name = "Alpha",
                configured = true,
                models = listOf(
                    ProviderModelWire(id = "m2", label = "M2"),
                    ProviderModelWire(id = "m1", label = "M1"),
                    ProviderModelWire(id = "m1", label = "M1 duplicate"),
                ),
            ),
            ProviderStatus(
                id = "beta",
                name = "Beta",
                configured = true,
                models = listOf(ProviderModelWire(id = "dup", label = "Dup"), ProviderModelWire(id = "a", label = "A")),
            ),
        )

        val next = reduce(baseState, ChatEvent.ModelCatalogLoaded(models, providers))

        assertEquals(
            listOf("z-local", "beta:dup", "qwen-local", "alpha:m1", "alpha:m2", "beta:a", "zeta:m1", "zeta:m2"),
            next.models.map { it.id },
        )
        assertSame(localZ, next.models[0])
        assertSame(localAlreadyPrefixed, next.models[1])
        assertSame(localA, next.models[2])
        assertNotSame(models, next.models)
        assertEquals(3, models.size)
        assertEquals(providers, next.providers)
    }

    @Test
    fun flagsArePreservedAcrossEveryEvent() {
        val events = listOf(
            ChatEvent.SessionListLoaded(listOf(ChatSession("s-a", "A", "local-a", "t1"))),
            ChatEvent.Delta("x"),
            ChatEvent.StreamDone(Source.Local),
            ChatEvent.ProvisioningUpdated(ProvisionState.Running(1L, 2L)),
            ChatEvent.ProvisioningUpdated(ProvisionState.Failed("synthetic failure")),
            ChatEvent.Unauthorized,
            ChatEvent.ModelCatalogLoaded(
                listOf(ModelEntry("local-only")),
                listOf(ProviderStatus("p", "P", configured = true)),
            ),
        )

        val on = baseState.copy(reducedMotion = true, compact = true)
        val off = baseState.copy(reducedMotion = false, compact = false)
        events.forEach { event ->
            val label = event::class.simpleName
            val fromOn = reduce(on, event)
            assertTrue("$label lost reducedMotion", fromOn.reducedMotion)
            assertTrue("$label lost compact", fromOn.compact)
            val fromOff = reduce(off, event)
            assertFalse("$label invented reducedMotion", fromOff.reducedMotion)
            assertFalse("$label invented compact", fromOff.compact)
        }
    }
}
