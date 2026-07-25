package com.vortex.a3.core.mirror

import android.app.ActivityManager
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log

/**
 * Background-safe launcher for the screen-mirror consent. When the laptop asks
 * to view this phone's screen, the phone must open [MirrorConsentActivity] to
 * get the one-shot MediaProjection token. If Vortex isn't in the foreground,
 * Android 10+ silently blocks that background activity start — so the request
 * appeared to "not arrive". This posts a high-priority **full-screen-intent**
 * notification instead (the same mechanism incoming-call screens use): the
 * system launches the consent directly when the device is awake, or shows a
 * heads-up the user taps to allow. Foreground? We start the activity directly.
 *
 * Its own module (per the per-feature split) — no [com.vortex.a3.service.VortexStack] growth.
 */
object MirrorRequestNotification {
    private const val TAG = "VortexMirror"
    private const val CHANNEL_ID = "vortex_mirror_req"
    private const val NOTIF_ID = 0x4D4952 // "MIR"

    /** Open the consent — directly if we're foreground, else via a full-screen
     *  notification that survives the background-activity-launch restriction. */
    fun prompt(ctx: Context) {
        val consent = Intent(ctx, com.vortex.a3.service.MirrorConsentActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (isForeground(ctx)) {
            try {
                ctx.startActivity(consent)
                return
            } catch (t: Throwable) {
                Log.w(TAG, "direct consent launch failed (${t.message}); falling back to notification")
            }
        }
        postFullScreen(ctx, consent)
    }

    /** Drop the request notification once consent has been handled. */
    fun clear(ctx: Context) {
        (ctx.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager)?.cancel(NOTIF_ID)
    }

    private fun postFullScreen(ctx: Context, consent: Intent) {
        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val ch = NotificationChannel(
                CHANNEL_ID, "Screen-share requests", NotificationManager.IMPORTANCE_HIGH,
            ).apply { description = "Laptop asking to view this phone's screen" }
            nm.createNotificationChannel(ch)
        }
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        val pi = PendingIntent.getActivity(ctx, NOTIF_ID, consent, flags)
        val notif = android.app.Notification.Builder(ctx, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_menu_share)
            .setContentTitle("Your laptop wants to view this screen")
            .setContentText("Tap to allow screen sharing")
            .setCategory(android.app.Notification.CATEGORY_CALL)
            .setAutoCancel(true)
            .setOngoing(false)
            .setContentIntent(pi)
            .setFullScreenIntent(pi, true)
            .build()
        try {
            nm.notify(NOTIF_ID, notif)
        } catch (t: Throwable) {
            Log.w(TAG, "mirror request notif failed: ${t.message}")
        }
    }

    /** True when a Vortex activity is actually visible (importance FOREGROUND).
     *  A bare foreground SERVICE reports FOREGROUND_SERVICE, which we treat as
     *  background here — its presence can't satisfy the activity-launch rule. */
    private fun isForeground(ctx: Context): Boolean {
        val am = ctx.getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager ?: return false
        val procs = am.runningAppProcesses ?: return false
        val pkg = ctx.packageName
        return procs.any {
            it.importance == ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND &&
                (it.processName == pkg || it.pkgList?.contains(pkg) == true)
        }
    }
}
