package com.vortex.a3.service

import android.app.Activity
import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.os.Bundle
import android.util.Log

/**
 * Transparent, no-UI activity that asks the user for the one-shot
 * MediaProjection consent ("Vortex wants to capture your screen"). The
 * resulting token can't be persisted across sessions (Android single-use), so
 * this runs each time the laptop starts a mirror.
 *
 * Flow: laptop sends START_MIRROR (BLE) → VortexStack sets [MirrorConsent.onResult]
 * and launches this activity → on grant we stash the token in [MirrorConsent]
 * and invoke the callback (VortexStack replies MIRROR_READY to the laptop, then
 * starts [ScreenMirrorService] once the Noise session's START params arrive).
 */
class MirrorConsentActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // We're launching the consent now — drop the full-screen request notif.
        com.vortex.a3.core.mirror.MirrorRequestNotification.clear(this)
        // Only ask on a FRESH create. A recreated activity — which is what
        // happens when the system rebuilds this transparent shell around the
        // system dialog — would otherwise fire a second capture request, and
        // the user would tap "Start now" only to be asked again. Two or three
        // prompts per mirror was this, not the laptop sending duplicates: the
        // laptop opens exactly one session and sends exactly one START.
        if (savedInstanceState != null) {
            return
        }
        try {
            val pm = getSystemService(MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
            @Suppress("DEPRECATION")
            startActivityForResult(pm.createScreenCaptureIntent(), REQ)
        } catch (t: Throwable) {
            Log.w(TAG, "consent launch failed: ${t.message}")
            MirrorConsent.deliver(false)
            finish()
        }
    }

    @Deprecated("startActivityForResult result callback")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == REQ && resultCode == RESULT_OK && data != null) {
            MirrorConsent.resultCode = resultCode
            MirrorConsent.resultData = data
            Log.i(TAG, "mirror consent granted")
            MirrorConsent.deliver(true)
        } else {
            Log.i(TAG, "mirror consent denied")
            MirrorConsent.clear()
            MirrorConsent.deliver(false)
        }
        finish()
    }

    companion object {
        private const val TAG = "VortexMirror"
        private const val REQ = 0x4D49 // "MI"

        fun launch(context: android.content.Context) {
            context.startActivity(
                Intent(context, MirrorConsentActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }
}

/**
 * Bridge between the consent activity and [com.vortex.a3.service.VortexStack].
 * Holds the short-lived MediaProjection token until the laptop's mirror session
 * is established and [ScreenMirrorService] consumes it.
 */
object MirrorConsent {
    @Volatile var resultCode: Int = 0
    @Volatile var resultData: Intent? = null

    /** Set by VortexStack before launching the activity; fired on grant/deny. */
    @Volatile var onResult: ((granted: Boolean) -> Unit)? = null

    @Volatile private var promptInFlight: Boolean = false
    @Volatile private var promptSince: Long = 0L

    fun hasToken(): Boolean = resultData != null

    /**
     * Debounce duplicate START frames (the laptop can briefly open two control
     * sessions): returns true only if no consent prompt is already pending, so a
     * second START doesn't pop a second "Start now" dialog. A stale flag older
     * than 10s is treated as cleared (so a genuinely new session still prompts).
     */
    @Synchronized
    fun beginPrompt(): Boolean {
        val now = android.os.SystemClock.elapsedRealtime()
        if (promptInFlight && now - promptSince < 10_000L) return false
        promptInFlight = true
        promptSince = now
        return true
    }

    @Synchronized
    fun deliver(granted: Boolean) {
        promptInFlight = false
        val cb = onResult
        onResult = null
        cb?.invoke(granted)
    }

    @Synchronized
    fun clear() {
        resultCode = 0
        resultData = null
    }
}
