package com.vortex.a3.core.media

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.vortex.a3.core.call.CallControl
import com.vortex.a3.service.VortexService

/**
 * The LAPTOP's now-playing on this phone: a compact custom-row notification
 * built from the `media_title/artist/app` fields of the laptop's AppState
 * heartbeat. Its ⏮⏯⏭ buttons queue a [VortexService.requestLaptopMedia]
 * transport command that rides the next snapshot back to the laptop
 * (seq-deduped there, executed via MPRIS).
 *
 * NOT a MediaStyle notification on purpose: MIUI renders MediaStyle at the
 * full media-carousel card height and that can't be shrunk, so we draw our
 * own single row (same DecoratedCustomViewStyle pattern as the persistent
 * Vortex notification).
 *
 * Mirror-image of the laptop's now-playing pill (the phone's media on the
 * laptop). A proxy [MediaSession] stays active alongside it so headset/buds
 * media keys still route to the laptop while it's the one playing — it never
 * plays audio, and the smart-switch's session scans exclude our own package
 * so the proxy can't confuse them.
 */
object LaptopMediaNotification {
    private const val TAG = "LaptopMedia"
    private const val CHANNEL_ID = "vortex_laptop_media"
    private const val NOTIF_ID = 0x4C4D5541 // "LMUA"
    private const val ACTION_MEDIA = "com.vortex.a3.LAPTOP_MEDIA"
    private const val EXTRA_OP = "op"

    /** Cancel the notification when the laptop stops updating us (link down /
     *  app gone). Heartbeats land every ~12s on both transports. */
    private const val STALE_MS = 45_000L

    private val main = Handler(Looper.getMainLooper())
    private val staleSweep = Runnable { clear() }

    @Volatile private var session: MediaSession? = null
    @Volatile private var receiverRegistered = false
    @Volatile private var appCtx: Context? = null
    /** What's currently shown, so unchanged heartbeats don't re-notify. */
    @Volatile private var shownKey: String? = null
    /** Last rendered content — lets a play/pause tap flip the ⏸/▶ icon
     *  OPTIMISTICALLY instead of waiting for the laptop's confirming
     *  heartbeat (which can be ~12s out). */
    @Volatile private var last: Shown? = null

    private data class Shown(
        val title: String,
        val artist: String,
        val app: String,
        val playing: Boolean,
    )
    /** "title|artist" the user swiped away — don't re-show the same track on
     *  the next heartbeat; a NEW track (or a cleared laptop) resets it. */
    @Volatile private var dismissedKey: String? = null

    /** Feed every inbound laptop AppState here. Empty [title] clears. */
    fun update(ctx: Context, title: String, artist: String, app: String, playing: Boolean) {
        val c = ctx.applicationContext
        appCtx = c
        if (title.isEmpty()) {
            main.removeCallbacks(staleSweep)
            clear()
            return
        }
        // Freshness window: cancel if the heartbeats stop arriving.
        main.removeCallbacks(staleSweep)
        main.postDelayed(staleSweep, STALE_MS)
        val key = "$title|$artist|$playing"
        if (dismissedKey == "$title|$artist") return // user swiped this track away
        if (shownKey == key) return // unchanged heartbeat → no re-notify
        try {
            show(c, title, artist, app, playing)
            shownKey = key
            last = Shown(title, artist, app, playing)
        } catch (t: Throwable) {
            Log.w(TAG, "show failed: ${t.message}")
        }
    }

    private fun show(ctx: Context, title: String, artist: String, app: String, playing: Boolean) {
        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            nm.createNotificationChannel(
                NotificationChannel(
                    CHANNEL_ID,
                    "Laptop media",
                    NotificationManager.IMPORTANCE_LOW,
                ).apply {
                    description = "Now playing on the paired laptop"
                    setSound(null, null)
                    enableVibration(false)
                },
            )
        }
        registerReceiver(ctx)
        // The session no longer backs the notification (no MediaStyle) but
        // stays active so buds/headset media keys reach the laptop.
        val s = ensureSession(ctx)
        // No ALBUM on purpose — MIUI renders it as an extra text row that
        // just repeats the player name and makes the card taller.
        s.setMetadata(
            MediaMetadata.Builder()
                .putString(MediaMetadata.METADATA_KEY_TITLE, title)
                .putString(MediaMetadata.METADATA_KEY_ARTIST, artist)
                .build(),
        )
        s.setPlaybackState(
            PlaybackState.Builder()
                .setState(
                    if (playing) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED,
                    PlaybackState.PLAYBACK_POSITION_UNKNOWN,
                    if (playing) 1f else 0f,
                )
                .setActions(
                    PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or
                        PlaybackState.ACTION_PLAY_PAUSE or
                        PlaybackState.ACTION_SKIP_TO_NEXT or
                        PlaybackState.ACTION_SKIP_TO_PREVIOUS,
                )
                .build(),
        )
        s.isActive = true

        fun actionPi(op: String, requestCode: Int): PendingIntent = PendingIntent.getBroadcast(
            ctx,
            requestCode,
            Intent(ACTION_MEDIA).setPackage(ctx.packageName).putExtra(EXTRA_OP, op),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val deletePi = PendingIntent.getBroadcast(
            ctx,
            4,
            Intent(ACTION_MEDIA).setPackage(ctx.packageName).putExtra(EXTRA_OP, "dismissed"),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val rv = android.widget.RemoteViews(
            ctx.packageName,
            com.vortex.a3.R.layout.notification_laptop_media,
        )
        rv.setTextViewText(com.vortex.a3.R.id.notification_media_title, title)
        rv.setTextViewText(
            com.vortex.a3.R.id.notification_media_artist,
            artist.ifEmpty { app },
        )
        rv.setImageViewResource(
            com.vortex.a3.R.id.notification_media_play_icon,
            if (playing) com.vortex.a3.R.drawable.ic_vortex_media_pause
            else com.vortex.a3.R.drawable.ic_vortex_media_play,
        )
        rv.setOnClickPendingIntent(
            com.vortex.a3.R.id.notification_media_prev,
            actionPi(CallControl.Action.MEDIA_PREV, 1),
        )
        rv.setOnClickPendingIntent(
            com.vortex.a3.R.id.notification_media_play,
            actionPi(CallControl.Action.MEDIA_PLAY_PAUSE, 2),
        )
        rv.setOnClickPendingIntent(
            com.vortex.a3.R.id.notification_media_next,
            actionPi(CallControl.Action.MEDIA_NEXT, 3),
        )

        val notif = androidx.core.app.NotificationCompat.Builder(ctx, CHANNEL_ID)
            .setSmallIcon(com.vortex.a3.R.drawable.ic_notification_vortex)
            .setColor(0xFF1AE76F.toInt())
            // Fallbacks for surfaces that ignore the custom view (a11y, some
            // lockscreens).
            .setContentTitle(title)
            .setContentText(artist.ifEmpty { app })
            .setCustomContentView(rv)
            .setStyle(androidx.core.app.NotificationCompat.DecoratedCustomViewStyle())
            .setPriority(androidx.core.app.NotificationCompat.PRIORITY_LOW)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setDeleteIntent(deletePi)
            .build()
        nm.notify(NOTIF_ID, notif)
    }

    /** The proxy session — created once; its transport callbacks (lockscreen /
     *  media-carousel buttons and headset keys while it's active) queue the
     *  same laptop command the notification buttons do. */
    private fun ensureSession(ctx: Context): MediaSession {
        session?.let { return it }
        synchronized(this) {
            session?.let { return it }
            val s = MediaSession(ctx, "vortex-laptop-media")
            s.setCallback(
                object : MediaSession.Callback() {
                    override fun onPlay() = sendOp(CallControl.Action.MEDIA_PLAY_PAUSE)
                    override fun onPause() = sendOp(CallControl.Action.MEDIA_PLAY_PAUSE)
                    override fun onSkipToNext() = sendOp(CallControl.Action.MEDIA_NEXT)
                    override fun onSkipToPrevious() = sendOp(CallControl.Action.MEDIA_PREV)
                },
                main,
            )
            session = s
            return s
        }
    }

    private fun sendOp(op: String) {
        val c = appCtx ?: return
        Log.i(TAG, "laptop media op: $op")
        VortexService.requestLaptopMedia(c, op)
        // Optimistic ⏸/▶ flip — the laptop's confirming heartbeat corrects
        // us within a second or two if the command didn't land.
        if (op == CallControl.Action.MEDIA_PLAY_PAUSE) {
            last?.let { s -> update(c, s.title, s.artist, s.app, !s.playing) }
        }
    }

    private fun registerReceiver(ctx: Context) {
        if (receiverRegistered) return
        synchronized(this) {
            if (receiverRegistered) return
            val r = object : BroadcastReceiver() {
                override fun onReceive(c: Context?, i: Intent?) {
                    when (val op = i?.getStringExtra(EXTRA_OP) ?: return) {
                        "dismissed" -> {
                            // Remember the track so heartbeats don't resurrect
                            // it; a new track (different key) shows again.
                            dismissedKey = shownKey?.substringBeforeLast('|')
                            shownKey = null
                            session?.isActive = false
                        }
                        else -> sendOp(op)
                    }
                }
            }
            val filter = IntentFilter(ACTION_MEDIA)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                ctx.registerReceiver(r, filter, Context.RECEIVER_NOT_EXPORTED)
            } else {
                @Suppress("UnspecifiedRegisterReceiverFlag")
                ctx.registerReceiver(r, filter)
            }
            receiverRegistered = true
        }
    }

    private fun clear() {
        shownKey = null
        dismissedKey = null
        last = null
        session?.isActive = false
        val c = appCtx ?: return
        (c.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager)
            ?.cancel(NOTIF_ID)
    }
}
