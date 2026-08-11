package com.vortex.a3.core.media

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.drawable.Icon
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.vortex.a3.core.call.CallControl
import com.vortex.a3.service.VortexService
import java.net.HttpURLConnection
import java.net.URL

/**
 * The LAPTOP's now-playing on this phone: a MediaStyle notification (system
 * media carousel + lockscreen) built from the `media_title/artist/app/art_url`
 * fields of the laptop's AppState heartbeat. Its ⏮⏯⏭ buttons queue a
 * [VortexService.requestLaptopMedia] transport command that rides the next
 * snapshot back to the laptop (seq-deduped there, executed via MPRIS).
 *
 * MediaStyle on purpose: MIUI renders it at the full media-carousel card
 * height, with the cover art drawn behind the title — the same card the
 * phone's own YouTube gets. 121fe9b once traded that away for a compact
 * custom row precisely BECAUSE the card can't be shrunk; the tall art-backed
 * card is what we want here, so that trade is reversed.
 *
 * Mirror-image of the laptop's now-playing pill (the phone's media on the
 * laptop). A proxy [MediaSession] backs the notification so the system renders
 * it as real media and headset/buds keys reach the laptop — it never plays
 * audio, and the smart-switch's session scans exclude our own package so the
 * proxy can't confuse them.
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

    /** Longest edge we keep cover art at. MediaSession ships the bitmap
     *  through a Binder transaction to every media consumer on the phone, so
     *  a full-size download would be both slow and refused. */
    private const val ART_MAX_PX = 512

    /** Refuse an art download bigger than this — the URL comes off the wire
     *  and a media card is never worth an unbounded read. */
    private const val ART_MAX_BYTES = 2 * 1024 * 1024

    private val main = Handler(Looper.getMainLooper())
    private val staleSweep = Runnable { clear() }

    @Volatile private var session: MediaSession? = null
    @Volatile private var receiverRegistered = false
    @Volatile private var appCtx: Context? = null
    /** What's currently shown, so unchanged heartbeats don't re-notify. */
    @Volatile private var shownKey: String? = null
    /** Last rendered content — lets a play/pause tap flip the ⏸/▶ icon
     *  OPTIMISTICALLY instead of waiting for the laptop's confirming
     *  heartbeat (which can be ~12s out), and lets the art fetch redraw the
     *  same card once the bitmap lands. */
    @Volatile private var last: Shown? = null

    /** One-entry art cache — only one track is ever on screen. */
    @Volatile private var artUrl: String? = null
    @Volatile private var artBitmap: Bitmap? = null
    /** URL a worker thread is currently fetching; nulled when it finishes. */
    @Volatile private var artFetching: String? = null

    private data class Shown(
        val title: String,
        val artist: String,
        val app: String,
        val artUrl: String,
        val playing: Boolean,
    )

    /** Feed every inbound laptop AppState here. Empty [title] clears. */
    fun update(
        ctx: Context,
        title: String,
        artist: String,
        app: String,
        artUrl: String,
        playing: Boolean,
    ) {
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
        val key = "$title|$artist|$playing|$artUrl"
        if (shownKey == key) return // unchanged heartbeat → no re-notify
        try {
            show(c, title, artist, app, artUrl, playing)
            shownKey = key
            last = Shown(title, artist, app, artUrl, playing)
        } catch (t: Throwable) {
            Log.w(TAG, "show failed: ${t.message}")
        }
    }

    private fun show(
        ctx: Context,
        title: String,
        artist: String,
        app: String,
        artUrl: String,
        playing: Boolean,
    ) {
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
        val s = ensureSession(ctx)
        // Null until the download lands; the fetch redraws this card when it
        // does, so the first beat of a new track shows text-then-art rather
        // than blocking the state handler on the network.
        val art = art(artUrl)
        // No ALBUM on purpose — MIUI renders it as an extra text row that
        // just repeats the player name.
        s.setMetadata(
            MediaMetadata.Builder()
                .putString(MediaMetadata.METADATA_KEY_TITLE, title)
                .putString(MediaMetadata.METADATA_KEY_ARTIST, artist.ifEmpty { app })
                // What MIUI's carousel draws behind the card.
                .putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, art)
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

        val style = Notification.MediaStyle()
            .setMediaSession(s.sessionToken)
            .setShowActionsInCompactView(0, 1, 2)
        val notif = Notification.Builder(ctx, CHANNEL_ID)
            .setSmallIcon(com.vortex.a3.R.drawable.ic_notification_vortex)
            // Real cover art when we have it; the brand spiral otherwise —
            // without SOMETHING here the style renders an empty gray square.
            .setLargeIcon(
                if (art != null) Icon.createWithBitmap(art)
                else Icon.createWithResource(ctx, com.vortex.a3.R.drawable.vortex_logo),
            )
            .setColor(0xFF1AE76F.toInt())
            .setContentTitle(title)
            .setContentText(artist.ifEmpty { app })
            .setStyle(style)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            // Not swipeable: this card is a live remote control, not an event.
            // It leaves ONLY when the laptop says the music stopped (empty
            // title) or the heartbeats dry up (the stale sweep) — so it can
            // never be lost mid-track and leave the ⏮⏯⏭ unreachable.
            .setOngoing(true)
            .addAction(
                Notification.Action.Builder(
                    Icon.createWithResource(ctx, com.vortex.a3.R.drawable.ic_vortex_media_prev),
                    "Previous", actionPi(CallControl.Action.MEDIA_PREV, 1),
                ).build(),
            )
            .addAction(
                Notification.Action.Builder(
                    Icon.createWithResource(
                        ctx,
                        if (playing) com.vortex.a3.R.drawable.ic_vortex_media_pause
                        else com.vortex.a3.R.drawable.ic_vortex_media_play,
                    ),
                    if (playing) "Pause" else "Play",
                    actionPi(CallControl.Action.MEDIA_PLAY_PAUSE, 2),
                ).build(),
            )
            .addAction(
                Notification.Action.Builder(
                    Icon.createWithResource(ctx, com.vortex.a3.R.drawable.ic_vortex_media_next),
                    "Next", actionPi(CallControl.Action.MEDIA_NEXT, 3),
                ).build(),
            )
            .setDeleteIntent(deletePi)
            .build()
        nm.notify(NOTIF_ID, notif)
    }

    /** Cached art for [url], kicking off the download on a miss. Null means
     *  "not yet" (or "never" — a failed fetch caches null so we don't retry
     *  the same dead URL on every 12s heartbeat). */
    private fun art(url: String): Bitmap? {
        if (url.isEmpty()) return null
        if (artUrl == url) return artBitmap
        synchronized(this) {
            if (artFetching == url) return null
            artFetching = url
        }
        Thread({
            val bmp = try {
                download(url)
            } catch (t: Throwable) {
                Log.w(TAG, "art fetch failed: ${t.message}")
                null
            }
            artUrl = url
            artBitmap = bmp
            artFetching = null
            // Redraw the SAME card so the art appears in place. Skipped if the
            // track moved on while we were downloading — that newer track's
            // own beat already drew it.
            main.post {
                val c = appCtx ?: return@post
                val s = last ?: return@post
                if (s.artUrl != url) return@post
                try {
                    show(c, s.title, s.artist, s.app, s.artUrl, s.playing)
                } catch (t: Throwable) {
                    Log.w(TAG, "art redraw failed: ${t.message}")
                }
            }
        }, "vortex-media-art").apply { isDaemon = true }.start()
        return null
    }

    /** GET [url] and decode it down to [ART_MAX_PX]. Off the main thread. */
    private fun download(url: String): Bitmap? {
        val conn = (URL(url).openConnection() as HttpURLConnection).apply {
            connectTimeout = 5_000
            readTimeout = 8_000
            instanceFollowRedirects = true
        }
        val bytes = try {
            if (conn.responseCode != HttpURLConnection.HTTP_OK) {
                Log.w(TAG, "art fetch HTTP ${conn.responseCode}")
                return null
            }
            conn.inputStream.use { it.readAtMost(ART_MAX_BYTES) } ?: return null
        } finally {
            conn.disconnect()
        }
        // Two-pass decode: measure, then sample down, so a big JPEG never
        // lands in memory at full size.
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        var sample = 1
        while (
            bounds.outWidth / sample > ART_MAX_PX * 2 ||
            bounds.outHeight / sample > ART_MAX_PX * 2
        ) {
            sample *= 2
        }
        val decoded = BitmapFactory.decodeByteArray(
            bytes, 0, bytes.size,
            BitmapFactory.Options().apply { inSampleSize = sample },
        ) ?: return null
        val longest = maxOf(decoded.width, decoded.height)
        if (longest <= ART_MAX_PX) return decoded
        val scale = ART_MAX_PX.toFloat() / longest
        return Bitmap.createScaledBitmap(
            decoded,
            (decoded.width * scale).toInt().coerceAtLeast(1),
            (decoded.height * scale).toInt().coerceAtLeast(1),
            true,
        )
    }

    /** Read the whole stream, or null if it runs past [cap]. */
    private fun java.io.InputStream.readAtMost(cap: Int): ByteArray? {
        val out = java.io.ByteArrayOutputStream()
        val buf = ByteArray(16 * 1024)
        while (true) {
            val n = read(buf)
            if (n < 0) return out.toByteArray()
            out.write(buf, 0, n)
            if (out.size() > cap) {
                Log.w(TAG, "art over ${cap}B; dropping")
                return null
            }
        }
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
            last?.let { s -> update(c, s.title, s.artist, s.app, s.artUrl, !s.playing) }
        }
    }

    private fun registerReceiver(ctx: Context) {
        if (receiverRegistered) return
        synchronized(this) {
            if (receiverRegistered) return
            val r = object : BroadcastReceiver() {
                override fun onReceive(c: Context?, i: Intent?) {
                    when (val op = i?.getStringExtra(EXTRA_OP) ?: return) {
                        // setOngoing should make this unreachable, but MIUI has
                        // its own ideas about what may be swiped. Forget what
                        // we drew so the very next heartbeat puts the card
                        // straight back instead of leaving the laptop's music
                        // running with no controls.
                        "dismissed" -> shownKey = null
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
        last = null
        session?.isActive = false
        val c = appCtx ?: return
        (c.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager)
            ?.cancel(NOTIF_ID)
    }
}
