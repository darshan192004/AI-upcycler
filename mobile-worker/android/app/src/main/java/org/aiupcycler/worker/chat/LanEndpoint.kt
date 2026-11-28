package org.aiupcycler.worker.chat

import android.content.SharedPreferences

/**
 * A manually entered or discovered master endpoint: host plus the LAN API port.
 *
 * Only these two non-secret values are written to the existing worker
 * preferences. The bearer token goes exclusively through [SecretStore] and is
 * never accepted by this type.
 */
data class LanEndpoint(val host: String, val port: Int) {
    init {
        require(host.isNotBlank()) { "LAN endpoint host must not be empty" }
        require(port in 1..65535) { "LAN endpoint port must be between 1 and 65535" }
    }

    fun baseUrl(): String = "http://$host:$port"

    companion object {
        const val PREF_HOST = "lan_host"
        const val PREF_PORT = "lan_port"

        /** Accepts only `host:port`; userinfo and out-of-range ports are refused. */
        fun parse(value: String): LanEndpoint {
            val input = value.trim()
            if (input.contains("@")) {
                throw IllegalArgumentException("LAN endpoint must not contain userinfo")
            }
            val separator = input.lastIndexOf(':')
            if (separator < 0) {
                throw IllegalArgumentException("LAN endpoint must be host:port")
            }
            val host = input.substring(0, separator).trim()
            if (host.isEmpty()) {
                throw IllegalArgumentException("LAN endpoint host must not be empty")
            }
            val port = input.substring(separator + 1).trim().toIntOrNull()
                ?: throw IllegalArgumentException("LAN endpoint port must be between 1 and 65535")
            if (port !in 1..65535) {
                throw IllegalArgumentException("LAN endpoint port must be between 1 and 65535")
            }
            return LanEndpoint(host, port)
        }
    }
}

/**
 * Narrow storage boundary: JVM tests use a recording fake, production uses
 * [SharedPreferencesLanEndpointStorage]. No Android type appears in the
 * validation or persistence logic.
 */
interface LanEndpointStorage {
    fun readString(key: String, defaultValue: String?): String?
    fun readInt(key: String, defaultValue: Int): Int
    fun writeString(key: String, value: String)
    fun writeInt(key: String, value: Int)
}

/** Adapter over the existing `ai-upcycler-worker` preferences file. */
class SharedPreferencesLanEndpointStorage(
    private val preferences: SharedPreferences,
) : LanEndpointStorage {
    override fun readString(key: String, defaultValue: String?): String? =
        preferences.getString(key, defaultValue)

    override fun readInt(key: String, defaultValue: Int): Int =
        preferences.getInt(key, defaultValue)

    override fun writeString(key: String, value: String) {
        preferences.edit().putString(key, value).apply()
    }

    override fun writeInt(key: String, value: Int) {
        preferences.edit().putInt(key, value).apply()
    }
}

/** Reads and writes only the two non-secret endpoint keys. */
class LanEndpointStore(private val storage: LanEndpointStorage) {
    fun read(): LanEndpoint? {
        val host = storage.readString(LanEndpoint.PREF_HOST, null) ?: return null
        if (host.isBlank()) return null
        val port = storage.readInt(LanEndpoint.PREF_PORT, -1)
        return LanEndpoint.parse("$host:$port")
    }

    fun save(endpoint: LanEndpoint) {
        storage.writeString(LanEndpoint.PREF_HOST, endpoint.host)
        storage.writeInt(LanEndpoint.PREF_PORT, endpoint.port)
    }
}
