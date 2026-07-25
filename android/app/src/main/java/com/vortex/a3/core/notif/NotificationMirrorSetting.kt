package com.vortex.a3.core.notif

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Whether this phone forwards its notifications to the paired laptop.
 * LOCAL-only (unlike the smart-switch, which is a shared cross-device
 * setting): it controls what THIS device sends, so there's nothing to
 * sync — turning it off just stops the outgoing forward. Process-wide
 * singleton over SharedPreferences `vortex_ui_settings`, default ON.
 */
object NotificationMirrorSetting {
    private const val PREFS = "vortex_ui_settings"
    private const val KEY = "notif_mirror_enabled"
    private const val KEY_SHOW_PEER = "notif_show_peer"

    private var prefs: SharedPreferences? = null
    private val _enabled = MutableStateFlow(true)
    private val _showPeer = MutableStateFlow(true)

    /** Observable: whether THIS phone forwards its notifications to the laptop. */
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    /** Observable: whether THIS phone shows the LAPTOP's mirrored notifications. */
    val showPeer: StateFlow<Boolean> = _showPeer.asStateFlow()

    /** Load persisted state. Idempotent. */
    @Synchronized
    fun init(context: Context) {
        if (prefs != null) return
        val p = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs = p
        _enabled.value = p.getBoolean(KEY, true)
        _showPeer.value = p.getBoolean(KEY_SHOW_PEER, true)
    }

    fun isEnabled(): Boolean = _enabled.value

    fun setEnabled(enabled: Boolean) {
        _enabled.value = enabled
        prefs?.edit()?.putBoolean(KEY, enabled)?.apply()
    }

    fun isShowPeer(): Boolean = _showPeer.value

    fun setShowPeer(show: Boolean) {
        _showPeer.value = show
        prefs?.edit()?.putBoolean(KEY_SHOW_PEER, show)?.apply()
    }
}
