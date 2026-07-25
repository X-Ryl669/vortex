package com.vortex.a3.core.media

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Smart audio-follow on/off — a SHARED, cross-device setting (unlike
 * locale/theme, which stay per-device). Process-wide singleton so the UI
 * (SettingsScreen) and the service (VortexStack) read/write one source of
 * truth: SharedPreferences `vortex_ui_settings`, keys `smart_switch_enabled`
 * + `smart_switch_changed_at`.
 *
 * Sync is last-writer-wins over the AppState heartbeat, keyed on the
 * change-timestamp — the exact mirror of the Linux `smart_switch_store` +
 * `MediaWatch.apply_setting`. Offline-tolerant by construction: a toggle
 * made while disconnected rides the next heartbeat once the link is back,
 * and the greater timestamp wins.
 */
object SmartSwitchSetting {
    private const val PREFS = "vortex_ui_settings"
    private const val K_ENABLED = "smart_switch_enabled"
    private const val K_TS = "smart_switch_changed_at"

    private var prefs: SharedPreferences? = null
    private val _enabled = MutableStateFlow(true)

    /** Observable on/off for the Settings UI + the service's coordinator. */
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    @Volatile
    private var changedAt: Long = 0L

    /** Load persisted state. Idempotent — the first caller (UI or service,
     *  whichever starts first) wins; later calls are no-ops. */
    @Synchronized
    fun init(context: Context) {
        if (prefs != null) return
        val p = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs = p
        _enabled.value = p.getBoolean(K_ENABLED, true)
        changedAt = p.getLong(K_TS, 0L)
    }

    fun isEnabled(): Boolean = _enabled.value
    fun changedAt(): Long = changedAt

    /** User toggled it locally. Stamp now (forced strictly greater than our
     *  own current ts so the toggle always wins + propagates), persist,
     *  publish. The caller should nudge the service so it pushes promptly. */
    fun setLocal(enabled: Boolean) {
        val now = System.currentTimeMillis() / 1000L
        val ts = maxOf(now, changedAt + 1)
        persist(enabled, ts)
    }

    /** Adopt the peer's value via LWW: only when its timestamp is strictly
     *  newer than ours (a `changedAt` of 0 means "no opinion", so any real
     *  peer toggle wins). Returns true if the value actually changed.
     *  Mirrors the Rust `MediaWatch::apply_setting`. */
    fun applyFromPeer(enabled: Boolean, ts: Long): Boolean {
        // Strictly-newer only. ts==0 ("no opinion") never wins, and two
        // zeros (neither side toggled yet) compare equal and are dropped —
        // which stops the every-heartbeat re-adoption at the default.
        if (ts <= changedAt) return false
        persist(enabled, ts)
        return true
    }

    private fun persist(enabled: Boolean, ts: Long) {
        changedAt = ts
        _enabled.value = enabled
        prefs?.edit()
            ?.putBoolean(K_ENABLED, enabled)
            ?.putLong(K_TS, ts)
            ?.apply()
    }
}
