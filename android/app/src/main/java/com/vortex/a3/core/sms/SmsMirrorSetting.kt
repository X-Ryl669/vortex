package com.vortex.a3.core.sms

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Whether this phone shares its recent SMS with the paired laptop's companion
 * Messages page. LOCAL-only (like the contacts/call-log mirrors); SharedPreferences
 * singleton, default ON.
 */
object SmsMirrorSetting {
    private const val PREFS = "vortex_ui_settings"
    private const val KEY = "sms_share_enabled"

    private var prefs: SharedPreferences? = null
    private val _enabled = MutableStateFlow(true)

    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

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
