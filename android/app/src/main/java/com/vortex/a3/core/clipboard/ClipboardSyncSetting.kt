package com.vortex.a3.core.clipboard

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Whether this phone takes part in clipboard sync with the paired laptop.
 * LOCAL-only (like the notification mirror): gates both the incoming apply
 * (laptop→phone setPrimaryClip) and the outgoing send (the Quick Settings
 * tile / quick-send). Process-wide singleton over SharedPreferences
 * `vortex_ui_settings`, default ON.
 */
object ClipboardSyncSetting {
    private const val PREFS = "vortex_ui_settings"
    private const val KEY = "clipboard_sync_enabled"

    private var prefs: SharedPreferences? = null
    private val _enabled = MutableStateFlow(true)

    /** Observable: whether clipboard sync is on. */
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    /** Load persisted state. Idempotent. */
    @Synchronized
    fun init(context: Context) {
        if (prefs != null) return
        val p = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs = p
        _enabled.value = p.getBoolean(KEY, true)
    }

    fun isEnabled(): Boolean = _enabled.value

    fun setEnabled(enabled: Boolean) {
        _enabled.value = enabled
        prefs?.edit()?.putBoolean(KEY, enabled)?.apply()
    }
}
