package com.vortex.a3.core.handoff

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import java.util.concurrent.atomic.AtomicLong

/**
 * "Open this on my phone" — the laptop→phone direction of handoff.
 *
 * The laptop stamps [com.vortex.a3.core.appstate.AppState.openOnPhone] with a
 * URL or a snippet and a monotonic unix-millis seq; we act on the RISING EDGE
 * only, exactly as [com.vortex.a3.core.ring.RingController] does, so the value
 * riding every heartbeat is idempotent and a reconnect does not re-open
 * anything.
 *
 * It posts a notification rather than opening the page itself. Android 10
 * blocks background activity starts, so opening directly would silently do
 * nothing from a background service — and a phone that opened pages because
 * another device said so is not a thing worth building even where the platform
 * allows it. One tap is the right amount of consent.
 */
object OpenOnPhone {
    private const val TAG = "VortexOpenOnPhone"
    private const val CHANNEL_ID = "vortex_open_on_phone"
    private const val NOTIF_ID = 0x4F50 // "OP"

    /** Last seq acted on. `-1` = nothing seen yet: the first snapshot after a
     *  start is ADOPTED, not acted on, so pairing or a restart never replays
     *  whatever the laptop happened to be carrying. */
    private val lastSeq = AtomicLong(-1L)

    /** Feed every inbound snapshot here. */
    fun onState(ctx: Context, text: String?, seq: Long) {
        if (seq <= 0L) return
        val prev = lastSeq.getAndSet(seq)
        if (prev < 0L) {
            Log.i(TAG, "adopting seq=$seq without acting (first snapshot)")
            return
        }
        if (seq <= prev) return
        val payload = text?.trim().orEmpty()
        if (payload.isEmpty()) return
        post(ctx, payload)
    }

    private fun post(ctx: Context, payload: String) {
        ensureChannel(ctx)
        // A URL opens in the browser; anything else goes to a share sheet, so
        // "send this text to my phone" lands somewhere useful instead of
        // failing to resolve.
        val looksUrl = payload.startsWith("http://") || payload.startsWith("https://")
        val intent = if (looksUrl) {
            Intent(Intent.ACTION_VIEW, android.net.Uri.parse(payload))
        } else {
            Intent.createChooser(
                Intent(Intent.ACTION_SEND).apply {
                    type = "text/plain"
                    putExtra(Intent.EXTRA_TEXT, payload)
                },
                null,
            )
        }.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

        val pi = PendingIntent.getActivity(
            ctx,
            payload.hashCode(),
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val n = NotificationCompat.Builder(ctx, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle(ctx.getString(com.vortex.a3.R.string.open_on_phone_title))
            .setContentText(payload)
            .setStyle(NotificationCompat.BigTextStyle().bigText(payload))
            .setAutoCancel(true)
            .setContentIntent(pi)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .build()
        try {
            NotificationManagerCompat.from(ctx).notify(NOTIF_ID, n)
            Log.i(TAG, "posted (url=$looksUrl)")
        } catch (_: SecurityException) {
            // POST_NOTIFICATIONS not granted — nothing to show.
        }
    }

    private fun ensureChannel(ctx: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val ch = NotificationChannel(
            CHANNEL_ID,
            ctx.getString(com.vortex.a3.R.string.open_on_phone_channel),
            NotificationManager.IMPORTANCE_HIGH,
        )
        ctx.getSystemService(NotificationManager::class.java)?.createNotificationChannel(ch)
    }
}
