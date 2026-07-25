package com.vortex.a3.core.contacts

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Whether this phone shares its contacts list with the paired laptop's
 * companion Contacts page. LOCAL-only (like [com.vortex.a3.core.notif.NotificationMirrorSetting]):
 * turning it off just stops the outgoing sync. SharedPreferences singleton,
 * default ON.
 */
object ContactsMirrorSetting {
    private const val PREFS = "vortex_ui_settings"
    private const val KEY = "contacts_share_enabled"

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
