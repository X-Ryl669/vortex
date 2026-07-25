package com.vortex.a3.core.ring

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.MediaPlayer
import android.media.RingtoneManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.util.Log

/**
 * Find-My-style "ring my phone". The laptop bumps `AppState.ringSeq` (unix-millis
 * of its Ring tap); we ring loudly on the RISING edge — routing the tone to the
 * ALARM stream at full alarm volume so it cuts through silent / vibrate / DND —
 * to help locate a misplaced phone. Auto-stops after [RING_MS]; a high-priority
 * notification with a Stop action cancels it early.
 *
 * Its own module (per the per-feature split): no [com.vortex.a3.service.VortexStack]
 * growth. Receive-only — this phone never rings the laptop.
 */
object RingController {
    private const val TAG = "VortexRing"
    private const val RING_MS = 25_000L
    private const val CHANNEL_ID = "vortex_ring"
    private const val NOTIF_ID = 0x52494E47 // "RING"
    private const val ACTION_STOP = "com.vortex.a3.RING_STOP"

    /** Last `ringSeq` we acted on. `-1` = nothing observed yet; the first value
     *  we see (even 0) only sets the baseline so a tap from BEFORE we connected
     *  never rings on connect. Monotonic millis from the laptop means a laptop
     *  restart (which resets its seq to 0) can't regress us into a false ring. */
    private var lastSeq = -1L

    @Volatile private var player: MediaPlayer? = null
    @Volatile private var vibrator: Vibrator? = null
    @Volatile private var savedAlarmVol = -1
    @Volatile private var stopReceiver: BroadcastReceiver? = null
    /** The full-screen "Found it!" page while it's showing (weak — never leaks). */
    @Volatile private var foundScreen: java.lang.ref.WeakReference<android.app.Activity>? = null
    private val main = Handler(Looper.getMainLooper())
    private val stopRunnable = Runnable { stop(appCtx) }
    @Volatile private var appCtx: Context? = null

    /** Feed every inbound `ringSeq` here. Rings on the rising edge only. */
    @Synchronized
    fun onRingSeq(context: Context, seq: Long) {
        if (lastSeq < 0L) {
            lastSeq = seq // first observation = baseline, never rings
            return
        }
        if (seq <= lastSeq) return
        lastSeq = seq
        ring(context.applicationContext)
    }

    private fun ring(ctx: Context) {
        appCtx = ctx
        Log.i(TAG, "ring: laptop asked to ring this phone")
        // Already ringing → just extend the auto-stop window (don't stack tones).
        if (player != null) {
            main.removeCallbacks(stopRunnable)
            main.postDelayed(stopRunnable, RING_MS)
            return
        }
        val am = ctx.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
        // Force the ALARM stream to full so silent/vibrate can't mute us. Alarm
        // volume needs no DND-policy access (unlike ringer mode), and USAGE_ALARM
        // is exempt from most DND filters — the right "find my phone" behaviour.
        if (am != null) {
            try {
                savedAlarmVol = am.getStreamVolume(AudioManager.STREAM_ALARM)
                val max = am.getStreamMaxVolume(AudioManager.STREAM_ALARM)
                am.setStreamVolume(AudioManager.STREAM_ALARM, max, 0)
            } catch (t: Throwable) {
                Log.w(TAG, "couldn't raise alarm volume: ${t.message}")
            }
        }
        val uri = RingtoneManager.getActualDefaultRingtoneUri(ctx, RingtoneManager.TYPE_ALARM)
            ?: RingtoneManager.getActualDefaultRingtoneUri(ctx, RingtoneManager.TYPE_RINGTONE)
            ?: RingtoneManager.getDefaultUri(RingtoneManager.TYPE_ALARM)
        try {
            player = MediaPlayer().apply {
                setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_ALARM)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                        .build(),
                )
                setDataSource(ctx, uri)
                isLooping = true
                setOnPreparedListener { it.start() }
                prepareAsync()
            }
        } catch (t: Throwable) {
            Log.w(TAG, "ring playback failed: ${t.message}")
        }
        startVibrate(ctx)
        showStopNotification(ctx)
        // Best-effort direct launch of the "Found it!" page. On a locked /
        // screen-off phone the notification's full-screen intent is the path
        // that's guaranteed to work; this direct start covers the in-use case
        // (background-activity-start may silently drop it elsewhere — fine).
        try {
            ctx.startActivity(
                Intent(ctx, RingActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        } catch (t: Throwable) {
            Log.w(TAG, "found-screen direct start failed: ${t.message}")
        }
        main.removeCallbacks(stopRunnable)
        main.postDelayed(stopRunnable, RING_MS)
    }

    /** [RingActivity] registers itself while visible so [stop] can close it. */
    fun attachFoundScreen(a: android.app.Activity) {
        foundScreen = java.lang.ref.WeakReference(a)
    }

    fun detachFoundScreen(a: android.app.Activity) {
        if (foundScreen?.get() === a) foundScreen = null
    }

    /** Stop ringing: silence the tone, restore alarm volume, drop the notif. */
    @Synchronized
    fun stop(ctx: Context?) {
        main.removeCallbacks(stopRunnable)
        player?.let {
            try { it.stop() } catch (_: Throwable) {}
            it.release()
        }
        player = null
        vibrator?.cancel()
        vibrator = null
        val c = ctx ?: appCtx
        if (c != null) {
            val am = c.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
            if (am != null && savedAlarmVol >= 0) {
                try { am.setStreamVolume(AudioManager.STREAM_ALARM, savedAlarmVol, 0) } catch (_: Throwable) {}
            }
            (c.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager)
                ?.cancel(NOTIF_ID)
            stopReceiver?.let { try { c.unregisterReceiver(it) } catch (_: Throwable) {} }
        }
        savedAlarmVol = -1
        stopReceiver = null
        // Close the "Found it!" page if it's up (auto-stop / notification Stop).
        foundScreen?.let { ref -> main.post { ref.get()?.finish() } }
        foundScreen = null
    }

    private fun startVibrate(ctx: Context) {
        val v = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            (ctx.getSystemService(Context.VIBRATOR_MANAGER_SERVICE) as? VibratorManager)?.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            ctx.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
        } ?: return
        vibrator = v
        val pattern = longArrayOf(0, 600, 400)
        try {
            v.vibrate(VibrationEffect.createWaveform(pattern, 0))
        } catch (t: Throwable) {
            Log.w(TAG, "vibrate failed: ${t.message}")
        }
    }

    /** A high-priority "Vortex is ringing — Stop" notification. Tapping Stop
     *  (or its action) silences the ring without unlocking into the app. */
    private fun showStopNotification(ctx: Context) {
        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val ch = NotificationChannel(CHANNEL_ID, "Find my phone", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "Plays a sound to help you locate this phone"
                setSound(null, null) // we drive the tone ourselves on the alarm stream
                enableVibration(false)
            }
            nm.createNotificationChannel(ch)
        }
        // Register the Stop receiver (idempotent — stop() unregisters it).
        if (stopReceiver == null) {
            val r = object : BroadcastReceiver() {
                override fun onReceive(c: Context?, i: Intent?) = stop(c ?: ctx)
            }
            stopReceiver = r
            val filter = IntentFilter(ACTION_STOP)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                ctx.registerReceiver(r, filter, Context.RECEIVER_NOT_EXPORTED)
            } else {
                @Suppress("UnspecifiedRegisterReceiverFlag")
                ctx.registerReceiver(r, filter)
            }
        }
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        val stopPi = PendingIntent.getBroadcast(
            ctx, 0, Intent(ACTION_STOP).setPackage(ctx.packageName), flags,
        )
        // Full-screen intent (alarm-style): on a locked or screen-off phone the
        // system launches the "Found it!" page immediately — the reliable path
        // where a background startActivity would be dropped.
        val foundPi = PendingIntent.getActivity(
            ctx, 1,
            Intent(ctx, RingActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            flags,
        )
        val notif = Notification.Builder(ctx, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_lock_idle_alarm)
            .setContentTitle("Vortex is ringing this phone")
            .setContentText("Tap Stop to silence")
            .setOngoing(true)
            .setCategory(Notification.CATEGORY_ALARM)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .addAction(Notification.Action.Builder(null, "Stop", stopPi).build())
            .setDeleteIntent(stopPi)
            .setFullScreenIntent(foundPi, true)
            .build()
        try {
            nm.notify(NOTIF_ID, notif)
        } catch (t: Throwable) {
            Log.w(TAG, "ring notif failed: ${t.message}")
        }
    }
}
