package com.vortex.a3.core.storage

import android.content.Context
import android.util.Base64
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import com.vortex.a3.core.crypto.X25519
import com.vortex.a3.core.identity.IDENTITY_VERSION
import com.vortex.a3.core.identity.IdentityRecord
import com.vortex.a3.core.identity.Platform

/**
 * EncryptedSharedPreferences-backed identity store per spec §3.2.
 *
 * The whole 90-byte Identity Record is stored as a single base64 value in
 * an [EncryptedSharedPreferences] file. The encryption keys are managed by
 * androidx.security and held in the Android Keystore (hardware-backed
 * where the device supports it; software-backed otherwise).
 *
 * If [MasterKey] generation fails (e.g., Keystore unavailable), construction
 * throws an exception so the caller can fail closed per spec §3.2.
 */
class EncryptedPrefsIdentityStore(context: Context) : IdentityStore {

    private val prefs = run {
        val masterKey = MasterKey.Builder(context, MASTER_KEY_ALIAS)
            .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
            .setRequestStrongBoxBacked(false) // emulators / older devices may lack StrongBox
            .build()
        EncryptedSharedPreferences.create(
            context.applicationContext,
            PREFS_FILENAME,
            masterKey,
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
        )
    }

    override fun save(record: IdentityRecord) {
        val bytes = record.encode()
        val b64 = Base64.encodeToString(bytes, Base64.NO_WRAP)
        prefs.edit().putString(KEY_IDENTITY, b64).apply()
    }

    override fun load(): IdentityRecord? {
        val b64 = prefs.getString(KEY_IDENTITY, null) ?: return null
        return decode(Base64.decode(b64, Base64.NO_WRAP))
    }

    override fun forget() {
        prefs.edit().remove(KEY_IDENTITY).apply()
    }

    private fun decode(bytes: ByteArray): IdentityRecord? {
        if (bytes.size != 90) return null
        val version = bytes[0]
        if (version != IDENTITY_VERSION) return null
        val deviceId = bytes.copyOfRange(1, 17)
        val staticPriv = bytes.copyOfRange(17, 49)
        val staticPub = bytes.copyOfRange(49, 81)
        val createdAt = java.nio.ByteBuffer.wrap(bytes, 81, 8).long
        val platformByte = bytes[89]
        val platform = Platform.fromByte(platformByte) ?: return null
        // Re-derive the public key as a sanity check; if it differs, the
        // stored record is corrupted.
        val expectedPub = X25519.publicFromPrivate(staticPriv)
        if (!staticPub.contentEquals(expectedPub)) return null
        return IdentityRecord(
            version = version,
            deviceId = deviceId,
            staticPriv = staticPriv,
            staticPub = staticPub,
            createdAt = createdAt,
            platform = platform,
        )
    }

    companion object {
        const val MASTER_KEY_ALIAS = "com.vortex.identity.v1.master"
        const val PREFS_FILENAME = "vortex_identity_v1"
        const val KEY_IDENTITY = "identity_record_b64"
    }
}
