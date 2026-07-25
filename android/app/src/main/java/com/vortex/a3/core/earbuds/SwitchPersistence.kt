package com.vortex.a3.core.earbuds

import android.content.Context
import android.util.Base64
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey

/**
 * Crash-recovery persistence for [SwitchOrchestrator] (R1 in
 * the earbuds-switch design notes §3).
 *
 * We persist the most-recent non-Idle [SwitchState] together with the
 * peer + mac + entry timestamp. On the next process start, [recover]
 * decides whether to **resume** an in-flight Connecting attempt,
 * **rollback** a stale flow to Idle, or **surface a brief Failed
 * message** so the user sees that the previous attempt did not
 * silently disappear.
 *
 * Stored in [EncryptedSharedPreferences] because the peer public key
 * is a long-lived identifier we do not want world-readable on disk.
 *
 * Wire format (single key/value):
 *   key   = "state.v1"
 *   value = base64(<discriminator>\n<reason>\n<enterMs>\n<peerHex>\n<mac>)
 *
 * The discriminator is a stable lowercase string so any future
 * version can recognize unknown variants and rollback rather than
 * crash. The blob is intentionally tiny (one row, overwritten on
 * every transition) — no log, no growth.
 */
class SwitchPersistence(context: Context) {

    private val prefs = run {
        val masterKey = MasterKey.Builder(context, MASTER_KEY_ALIAS)
            .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
            .build()
        EncryptedSharedPreferences.create(
            context.applicationContext,
            PREFS_FILENAME,
            masterKey,
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
        )
    }

    /** What's currently persisted, or null if Idle / never written. */
    data class Saved(
        val discriminator: String,
        val reason: String?,
        val enterMs: Long,
        val peerPub: ByteArray?,
        val mac: String?,
    ) {
        override fun equals(other: Any?): Boolean {
            if (this === other) return true
            if (other !is Saved) return false
            return discriminator == other.discriminator &&
                reason == other.reason &&
                enterMs == other.enterMs &&
                (peerPub?.contentEquals(other.peerPub) ?: (other.peerPub == null)) &&
                mac == other.mac
        }
        override fun hashCode(): Int {
            var r = discriminator.hashCode()
            r = 31 * r + (reason?.hashCode() ?: 0)
            r = 31 * r + enterMs.hashCode()
            r = 31 * r + (peerPub?.contentHashCode() ?: 0)
            r = 31 * r + (mac?.hashCode() ?: 0)
            return r
        }
    }

    /** Recovery action [SwitchOrchestrator] should take on init. */
    sealed class Action {
        object None : Action()
        data class Rollback(val previousReason: String) : Action()
        data class ResumeConnect(val peerPub: ByteArray, val mac: String) : Action()
    }

    fun save(state: SwitchState, peerPub: ByteArray?, mac: String?) {
        val disc = state.discriminator()
        val reason = (state as? SwitchState.Failed)?.reason
        val peerHex = peerPub?.toHex().orEmpty()
        val macStr = mac.orEmpty()
        val raw = listOf(disc, reason.orEmpty(), System.currentTimeMillis().toString(), peerHex, macStr)
            .joinToString("\n")
        prefs.edit()
            .putString(KEY, Base64.encodeToString(raw.toByteArray(Charsets.UTF_8), Base64.NO_WRAP))
            .apply()
    }

    fun clear() {
        prefs.edit().remove(KEY).apply()
    }

    fun load(): Saved? {
        val b64 = prefs.getString(KEY, null) ?: return null
        return runCatching {
            val s = String(Base64.decode(b64, Base64.NO_WRAP), Charsets.UTF_8)
            val parts = s.split('\n', limit = 5)
            if (parts.size < 5) return null
            val peerPub = parts[3].takeIf { it.isNotEmpty() }?.fromHexOrNull()
            Saved(
                discriminator = parts[0],
                reason = parts[1].ifEmpty { null },
                enterMs = parts[2].toLong(),
                peerPub = peerPub,
                mac = parts[4].ifEmpty { null },
            )
        }.getOrNull()
    }

    /** Decide what to do based on the persisted state on app start.
     *  Caller (orchestrator init path) must clear the slot after
     *  acting on the result. */
    fun recover(nowMs: Long = System.currentTimeMillis()): Action {
        val s = load() ?: return Action.None
        if (s.discriminator == DISC_IDLE) return Action.None
        val age = nowMs - s.enterMs
        if (age > STALE_TTL_MS) {
            return Action.Rollback("previous switch stale (${age / 1000}s); rolled back")
        }
        return when (s.discriminator) {
            DISC_CONNECTING -> {
                val peer = s.peerPub
                val mac = s.mac
                if (age <= RESUME_CONNECT_MAX_MS && peer != null && !mac.isNullOrEmpty()) {
                    Action.ResumeConnect(peer, mac)
                } else {
                    Action.Rollback("previous Connecting too old or missing context")
                }
            }
            DISC_FAILED -> Action.Rollback(s.reason ?: "previous switch failed")
            // WaitingApproval, WaitingReleased, Preparing — peer's view
            // already moved on. Honest thing is rollback with reason.
            else -> Action.Rollback("previous switch interrupted (${s.discriminator})")
        }
    }

    companion object {
        const val MASTER_KEY_ALIAS: String = "com.vortex.switch.v1.master"
        const val PREFS_FILENAME: String = "vortex_switch_state_v1"
        private const val KEY: String = "state.v1"

        // Discriminators — kept stable; if a state is renamed the
        // discriminator stays so old persisted blobs still decode.
        const val DISC_IDLE: String = "idle"
        const val DISC_PREPARING: String = "preparing"
        const val DISC_WAITING_APPROVAL: String = "waiting_approval"
        const val DISC_WAITING_RELEASED: String = "waiting_released"
        const val DISC_CONNECTING: String = "connecting"
        const val DISC_FAILED: String = "failed"

        /** Resume window: if a Connecting state is newer than this, we
         *  retry the BT connect — peer already released, so re-attempt
         *  is idempotent (already-connected = no-op). */
        const val RESUME_CONNECT_MAX_MS: Long = 10_000

        /** Anything older than this is unconditional rollback. Above
         *  this age the wire protocol context is already gone. */
        const val STALE_TTL_MS: Long = 30_000
    }
}

private fun SwitchState.discriminator(): String = when (this) {
    SwitchState.Idle -> SwitchPersistence.DISC_IDLE
    SwitchState.Preparing -> SwitchPersistence.DISC_PREPARING
    SwitchState.WaitingApproval -> SwitchPersistence.DISC_WAITING_APPROVAL
    SwitchState.WaitingReleased -> SwitchPersistence.DISC_WAITING_RELEASED
    SwitchState.Connecting -> SwitchPersistence.DISC_CONNECTING
    // AlmostDone is the optimistic-UI variant of Connecting: BT is
    // still settling, so for persistence we treat it as Connecting
    // and resume the connect attempt if the process is restarted.
    SwitchState.AlmostDone -> SwitchPersistence.DISC_CONNECTING
    is SwitchState.Failed -> SwitchPersistence.DISC_FAILED
}

private fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }

private fun String.fromHexOrNull(): ByteArray? {
    if (length % 2 != 0) return null
    return runCatching {
        ByteArray(length / 2) { i ->
            ((Character.digit(this[2 * i], 16) shl 4) +
                Character.digit(this[2 * i + 1], 16)).toByte()
        }
    }.getOrNull()
}
