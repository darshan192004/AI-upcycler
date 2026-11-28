package org.aiupcycler.worker.chat.secrets

/**
 * The boundary between chat code and whatever protects its secrets.
 *
 * The JVM unit-test lane uses [MemorySecretStore]; Android uses
 * [KeystoreSecretStore]. Neither implementation logs values, and debug output
 * from a store must never include a key's contents.
 */
interface SecretStore {
    fun read(key: String): String?
    fun write(key: String, value: String)
    fun delete(key: String)

    companion object {
        /** The one non-provider key: the bearer token for the master's LAN API. */
        const val LAN_TOKEN_KEY = "lan_token"

        /** Prefix for provider API keys, so they cannot collide with other keys. */
        const val PROVIDER_PREFIX = "provider:"

        /** The four provider ids accepted by sub-project A's routes. */
        val PROVIDERS = listOf("openai", "anthropic", "gemini", "kimi")

        /**
         * The exact storage key for an id. Provider ids become
         * `provider:<id>`; the LAN token keeps its documented key unchanged.
         *
         * @throws IllegalArgumentException for any other id.
         */
        fun keyFor(provider: String): String = when (provider) {
            LAN_TOKEN_KEY -> LAN_TOKEN_KEY
            in PROVIDERS -> "$PROVIDER_PREFIX$provider"
            else -> throw IllegalArgumentException("unknown provider: $provider")
        }
    }
}

/**
 * In-memory store for JVM tests and for callers that have no Android Keystore.
 * Values are deliberately excluded from [toString].
 */
class MemorySecretStore : SecretStore {
    private val values = mutableMapOf<String, String>()

    @Synchronized
    override fun read(key: String): String? = values[this.canonicalKey(key)]

    @Synchronized
    override fun write(key: String, value: String) {
        values[this.canonicalKey(key)] = value
    }

    @Synchronized
    override fun delete(key: String) {
        values.remove(this.canonicalKey(key))
    }

    override fun toString(): String = "MemorySecretStore(keys=${values.keys.sorted()})"
}

/** Shared key normalization for the Android and JVM implementations. */
internal fun SecretStore.canonicalKey(key: String): String = when {
    key == SecretStore.LAN_TOKEN_KEY -> key
    key.startsWith(SecretStore.PROVIDER_PREFIX) ->
        SecretStore.keyFor(key.removePrefix(SecretStore.PROVIDER_PREFIX))
    else -> SecretStore.keyFor(key)
}
