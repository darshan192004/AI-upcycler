package org.aiupcycler.worker.chat.secrets

import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey

/**
 * Android implementation of [SecretStore].
 *
 * The master key lives in the Android Keystore under a stable alias; keys and
 * values in the backing preferences file are encrypted with AES256-SIV and
 * AES256-GCM respectively, so neither provider API keys nor the LAN bearer
 * token appear in plaintext on disk. Values are never logged.
 */
class KeystoreSecretStore(context: Context) : SecretStore {
    private val preferences: SharedPreferences = EncryptedSharedPreferences.create(
        context.applicationContext,
        PREFERENCES_FILE,
        MasterKey.Builder(context.applicationContext, MASTER_KEY_ALIAS)
            .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
            .build(),
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
    )

    override fun read(key: String): String? = preferences.getString(this.canonicalKey(key), null)

    override fun write(key: String, value: String) {
        preferences.edit().putString(this.canonicalKey(key), value).apply()
    }

    override fun delete(key: String) {
        preferences.edit().remove(this.canonicalKey(key)).apply()
    }

    private companion object {
        const val MASTER_KEY_ALIAS = "ai-upcycler-chat"
        const val PREFERENCES_FILE = "ai-upcycler-chat-secrets"
    }
}
