package com.vortex.a3.core.media

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Whether a screenshot / a camera photo taken on this phone is sent to the
 * paired laptop by itself, the moment it lands in the gallery.
 *
 * Two switches, not one. A screenshot is small and dull and people want it on
 * the laptop to paste into a chat; a photo is large and personal. Someone who
 * wants the first will often not want the second, and a single switch would
 * make them choose between "nothing" and "everything I photograph".
 *
 * Both default **OFF**, and that is not a tuning choice. This is the one
 * feature in the app that copies a person's pictures to another machine
 * without them lifting a finger, so it only ever runs because they turned it
 * on. Not inferred from clipboard sync, not inferred from file auto-accept,
 * not turned on by a permission grant.
 *
 * LOCAL-only, like [com.vortex.a3.core.lan.FileAutoAcceptSetting]: the data
 * and the storage permission both live on the phone, so this is where the
 * decision lives. The laptop keeps its own say through its receive consent.
 * Process-wide singleton over SharedPreferences `vortex_ui_settings`.
 */
object MediaAutoShareSetting {
    private const val PREFS = "vortex_ui_settings"
    private const val K_SCREENSHOTS = "media_share_screenshots"
    private const val K_PHOTOS = "media_share_photos"

    private var prefs: SharedPreferences? = null
    private val _screenshots = MutableStateFlow(false)
    private val _photos = MutableStateFlow(false)

    /** Observable: send new screenshots to the laptop. */
    val screenshots: StateFlow<Boolean> = _screenshots.asStateFlow()

    /** Observable: send new camera photos to the laptop. */
    val photos: StateFlow<Boolean> = _photos.asStateFlow()

    /** Load persisted state. Idempotent — safe from the UI and the service. */
    @Synchronized
    fun init(context: Context) {
        if (prefs != null) return
        val p = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs = p
        _screenshots.value = p.getBoolean(K_SCREENSHOTS, false)
        _photos.value = p.getBoolean(K_PHOTOS, false)
    }

    fun screenshotsEnabled(): Boolean = _screenshots.value
    fun photosEnabled(): Boolean = _photos.value

    /** Either switch on — the only case worth registering an observer for. */
    fun anyEnabled(): Boolean = _screenshots.value || _photos.value

    fun setScreenshots(enabled: Boolean) {
        _screenshots.value = enabled
        prefs?.edit()?.putBoolean(K_SCREENSHOTS, enabled)?.apply()
    }

    fun setPhotos(enabled: Boolean) {
        _photos.value = enabled
        prefs?.edit()?.putBoolean(K_PHOTOS, enabled)?.apply()
    }
}
