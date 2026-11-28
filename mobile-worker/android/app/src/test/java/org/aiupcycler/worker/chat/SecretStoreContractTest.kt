package org.aiupcycler.worker.chat

import org.aiupcycler.worker.chat.secrets.MemorySecretStore
import org.aiupcycler.worker.chat.secrets.SecretStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

/**
 * The JVM-side secret-store contract (plan task 7): exact key naming, a single
 * non-provider key for the LAN bearer token, and no secret in debug output.
 */
class SecretStoreContractTest {

    @Test
    fun keyForUsesTheExactProviderAndLanKeys() {
        assertEquals("provider:openai", SecretStore.keyFor("openai"))
        assertEquals("provider:anthropic", SecretStore.keyFor("anthropic"))
        assertEquals("provider:gemini", SecretStore.keyFor("gemini"))
        assertEquals("provider:kimi", SecretStore.keyFor("kimi"))
        assertEquals("lan_token", SecretStore.keyFor("lan_token"))
    }

    @Test
    fun keyForRejectsAnythingOutsideTheFourProviderIds() {
        val error = assertThrows(IllegalArgumentException::class.java) {
            SecretStore.keyFor("not-a-provider")
        }
        assertEquals("unknown provider: not-a-provider", error.message)
    }

    @Test
    fun memoryStoreRoundTripsAndDeletesSecrets() {
        val store = MemorySecretStore()

        store.write(SecretStore.keyFor("openai"), "sk-test-openai")
        store.write(SecretStore.LAN_TOKEN_KEY, "token-test-memory")

        assertEquals("sk-test-openai", store.read(SecretStore.keyFor("openai")))
        assertEquals("token-test-memory", store.read(SecretStore.LAN_TOKEN_KEY))
        store.delete(SecretStore.keyFor("openai"))
        store.delete(SecretStore.LAN_TOKEN_KEY)
        assertNull(store.read(SecretStore.keyFor("openai")))
        assertNull(store.read(SecretStore.LAN_TOKEN_KEY))
    }

    @Test
    fun debugOutputNeverContainsAStoredValue() {
        val store = MemorySecretStore()
        store.write(SecretStore.keyFor("anthropic"), "sk-test-do-not-print")
        store.write(SecretStore.LAN_TOKEN_KEY, "token-test-do-not-print")

        val debug = store.toString()
        assertFalse(debug.contains("sk-test-do-not-print"))
        assertFalse(debug.contains("token-test-do-not-print"))
    }
}
