package com.vortex.a3.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import android.view.Surface
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.nio.ByteBuffer

/**
 * Screen-mirror SENDER (phone → laptop). MediaProjection → VirtualDisplay
 * (AUTO_MIRROR, captures the lock screen too) → MediaCodec H.264 (Baseline,
 * realtime) → AVCC→Annex-B → sealed **TCP** video to the laptop.
 *
 * Adapted from the upstream `ecosystem` prototype (which, like scrcpy, sends
 * video over TCP for reliability), keeping Vortex's encryption:
 *  - The phone runs a **TCP video server** on [MIRROR_VIDEO_PORT]; the laptop
 *    connects out to it (its firewall blocks unsolicited inbound but allows
 *    established). Each H.264 access unit is sealed whole with
 *    ChaCha20-Poly1305 ([com.vortex.a3.core.mirror.MirrorTcpSealer]) and
 *    length-prefixed. TCP is ordered + reliable, so a frame is never lost —
 *    no UDP-style burst-loss freezes.
 *  - Control (keyframe requests, ping/pong, stop) rides the encrypted TCP Noise
 *    session (`MirrorSession`); the laptop's keyframe request arrives as
 *    [ACTION_REQUEST_KEYFRAME].
 *
 * FGS ordering (Android 14+): `startForeground(..., MEDIA_PROJECTION)` happens
 * BEFORE `getMediaProjection`, and a `MediaProjection.Callback` is registered.
 *
 * The encoder params + media key arrive via Intent extras (the caller in
 * `VortexStack` has them once the Noise session's START frame is parsed and the
 * consent token is in hand).
 */
class ScreenMirrorService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val mainHandler = Handler(Looper.getMainLooper())

    private var screenWakeLock: android.os.PowerManager.WakeLock? = null
    private var projection: MediaProjection? = null
    private var virtualDisplay: VirtualDisplay? = null
    private var codec: MediaCodec? = null
    private var inputSurface: Surface? = null

    // TCP video server (reliable + ordered, like scrcpy): the phone listens, the
    // laptop connects out (its firewall blocks inbound but allows established).
    private var videoServer: java.net.ServerSocket? = null
    private var videoClient: java.net.Socket? = null
    private var videoOut: java.io.OutputStream? = null
    private var tcpSealer: com.vortex.a3.core.mirror.MirrorTcpSealer? = null
    @Volatile private var clientConnected = false
    private val writeLock = Any()

    /** `System.nanoTime()` at which the in-flight socket write started, 0 when
     *  none is. Read by the watchdog from another thread. */
    @Volatile private var writeStartedNs = 0L

    private var encoderJob: Job? = null

    @Volatile private var running = false
    @Volatile private var bitrate: Int = 6_000_000        // ceiling (requested)
    @Volatile private var currentBitrate: Int = 6_000_000 // live adaptive value
    // Adaptive-bitrate (congestion control) state — touched only under writeLock.
    private var adaptWindowStartNs: Long = 0
    private var adaptBlockedNs: Long = 0
    @Volatile private var framesSent: Long = 0
    @Volatile private var codecConfigAnnexB: ByteArray? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> scope.launch { startMirroring(intent) }
            ACTION_REQUEST_KEYFRAME -> requestSyncFrame()
            ACTION_STOP -> stopSelf()
        }
        return START_STICKY
    }

    override fun onDestroy() {
        running = false
        encoderJob?.cancel()
        scope.cancel()
        releaseMirrorPipeline()
        super.onDestroy()
    }

    private fun startMirroring(intent: Intent) {
        try {
            if (running) {
                // Restart cleanly instead of ignoring: a stale session would
                // keep port 51822 + the old client, so the new laptop connection
                // collides and resets → the user sees a freeze. Tear the old one
                // down fully, then fall through to start fresh.
                Log.i(TAG, "mirror: restart — tearing down previous session")
                running = false
                encoderJob?.cancel()
                releaseMirrorPipeline()
                clientConnected = false
                codecConfigAnnexB = null
                adaptWindowStartNs = 0
                adaptBlockedNs = 0
            }
            val resultCode = intent.getIntExtra(EXTRA_RESULT_CODE, Int.MIN_VALUE)
            val resultData =
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableExtra(EXTRA_RESULT_DATA, Intent::class.java)
                } else {
                    @Suppress("DEPRECATION") intent.getParcelableExtra(EXTRA_RESULT_DATA)
                }
            val width = (intent.getIntExtra(EXTRA_WIDTH, 0) / 2) * 2
            val height = (intent.getIntExtra(EXTRA_HEIGHT, 0) / 2) * 2
            val fps = intent.getIntExtra(EXTRA_FPS, 30).coerceIn(15, 60)
            bitrate = intent.getIntExtra(EXTRA_BITRATE, 6_000_000)
            // Start below the ceiling and let adaptive bitrate ramp up — like
            // congestion-control slow start; avoids an initial overshoot freeze.
            currentBitrate = (bitrate * ADAPT_START_FRAC).toInt().coerceAtLeast(ADAPT_MIN_BITRATE)
            val key = intent.getByteArrayExtra(EXTRA_KEY)
            if (resultCode == Int.MIN_VALUE || resultData == null ||
                width == 0 || height == 0 || key == null || key.size != 32
            ) {
                Log.w(TAG, "mirror config invalid; stopping")
                stopSelf()
                return
            }

            // FGS BEFORE getMediaProjection (Android 14+ requirement).
            createNotificationChannel()
            val notif = buildNotification("Mirroring to laptop")
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(NOTIFICATION_ID, notif, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
            } else {
                startForeground(NOTIFICATION_ID, notif)
            }

            @Suppress("DEPRECATION")
            screenWakeLock = (getSystemService(Context.POWER_SERVICE) as android.os.PowerManager)
                .newWakeLock(
                    android.os.PowerManager.SCREEN_DIM_WAKE_LOCK or
                        android.os.PowerManager.ACQUIRE_CAUSES_WAKEUP,
                    "VortexMirror:screen",
                ).also { it.acquire(60 * 60 * 1000L) }

            val pm = getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
            val proj = pm.getMediaProjection(resultCode, resultData)
            if (proj == null) {
                Log.w(TAG, "projection denied")
                stopSelf()
                return
            }
            // Android 14 throws if no callback is registered before capture.
            proj.registerCallback(object : MediaProjection.Callback() {
                override fun onStop() {
                    running = false
                    stopSelf()
                }
            }, mainHandler)
            projection = proj

            tcpSealer = com.vortex.a3.core.mirror.MirrorTcpSealer(key)
            // TCP video server: bind the fixed port; the laptop connects out to
            // us. The accept loop is launched below, AFTER `running = true`, so
            // its `while (running …)` guard doesn't see a stale false and exit.
            videoServer = java.net.ServerSocket().apply {
                reuseAddress = true
                bind(java.net.InetSocketAddress(MIRROR_VIDEO_PORT))
                soTimeout = VIDEO_ACCEPT_TIMEOUT_MS
            }

            configureEncoder(width, height, fps)

            val vdFlags = DisplayManager.VIRTUAL_DISPLAY_FLAG_PUBLIC or
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR
            virtualDisplay = proj.createVirtualDisplay(
                "VortexMirror", width, height, resources.displayMetrics.densityDpi,
                vdFlags, inputSurface, null, mainHandler,
            )
            if (virtualDisplay == null) {
                Log.w(TAG, "virtual display failed")
                stopSelf()
                return
            }

            running = true
            framesSent = 0
            updateStatus("Streaming ${width}x${height}@${fps} (TCP :$MIRROR_VIDEO_PORT)")
            // Accept the laptop (it connects out to us). Its own coroutine so the
            // encoder can spin up meanwhile; frames are dropped until connected,
            // then we force an immediate keyframe so the laptop decodes at once.
            scope.launch { acceptVideoClient() }
            encoderJob = scope.launch { encoderLoop() }
        } catch (t: Throwable) {
            Log.e(TAG, "startMirroring failed", t)
            stopSelf()
        }
    }

    private fun configureEncoder(width: Int, height: Int, fps: Int) {
        // HEVC (H.265): ~40% better quality-per-bit than H.264 — the go-to
        // screen-mirroring codec. Same hardware-surface encode path; the laptop
        // decodes it on the GPU (nvh265dec). The Annex-B / CSD plumbing below is
        // codec-agnostic (HEVC just packs VPS+SPS+PPS into csd-0, csd-1 null).
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_HEVC, width, height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, currentBitrate)
            setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
            setInteger(MediaFormat.KEY_FRAME_RATE, fps)
            // Long GOP: TCP never loses a frame, so periodic IDRs only add
            // bandwidth spikes (a recurring hitch). One IDR at start (forced on
            // connect) + on-request is enough; 60 s keeps a rare resync point.
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 60)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                setInteger(MediaFormat.KEY_PRIORITY, 0)
                // HEVC Main profile, level 4.1 (covers 1080p up to 60 fps).
                setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.HEVCProfileMain)
                setInteger(MediaFormat.KEY_LEVEL, MediaCodecInfo.CodecProfileLevel.HEVCMainTierLevel41)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                setFloat(MediaFormat.KEY_OPERATING_RATE, fps.toFloat())
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                setInteger(MediaFormat.KEY_LATENCY, 0)
                setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
                // NOTE: KEY_MAX_FPS_TO_ENCODER (encoder-input cap) was tried to
                // bound high-refresh (120 Hz) phones but made THIS encoder drop
                // frames unevenly → multi-second stalls. Left out; the encoder
                // runs at its natural rate, which is smooth here.
            }
        }
        codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_HEVC).apply {
            configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            inputSurface = createInputSurface()
            start()
        }
    }

    private fun encoderLoop() {
        val localCodec = codec ?: return
        val info = MediaCodec.BufferInfo()
        while (scope.isActive && running) {
            val index = try {
                localCodec.dequeueOutputBuffer(info, 10_000)
            } catch (t: Throwable) {
                Log.e(TAG, "encoder dequeue failed", t)
                break
            }
            when {
                index >= 0 -> {
                    val buf = localCodec.getOutputBuffer(index)
                    if (buf != null && info.size > 0) {
                        buf.position(info.offset)
                        buf.limit(info.offset + info.size)
                        val au = ByteArray(info.size)
                        buf.get(au)
                        val isKeyframe = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                        val normalized = normalizeAccessUnit(au, isKeyframe)
                        if (normalized.isNotEmpty()) sendAccessUnit(normalized)
                        framesSent++
                        if (framesSent % 300L == 0L) updateStatus("frames=$framesSent")
                    }
                    localCodec.releaseOutputBuffer(index, false)
                }
                index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    val fmt = localCodec.outputFormat
                    val csd0 = fmt.getByteBuffer("csd-0")?.toByteArray()
                    val csd1 = fmt.getByteBuffer("csd-1")?.toByteArray()
                    val config = buildCodecConfigAnnexB(csd0, csd1)
                    codecConfigAnnexB = config.takeIf { it.isNotEmpty() }
                    if (config.isNotEmpty()) sendAccessUnit(config)
                }
            }
        }
        running = false
        stopSelf()
    }

    /** Accept the laptop's outbound video connection, then force a keyframe so
     *  it can start decoding immediately. Retries within the service lifetime. */
    private fun acceptVideoClient() {
        val server = videoServer ?: return
        while (scope.isActive && running && !clientConnected) {
            try {
                val client = server.accept()
                client.tcpNoDelay = true                 // Nagle off — low latency
                client.sendBufferSize = VIDEO_SEND_BUFFER // small → backpressure paces the encoder
                videoClient = client
                videoOut = java.io.BufferedOutputStream(client.getOutputStream(), VIDEO_SEND_BUFFER)
                clientConnected = true
                startWriteWatchdog()
                Log.i(TAG, "mirror: laptop video client connected ${client.inetAddress?.hostAddress}")
                // Push SPS/PPS + an immediate IDR so the decoder locks on at once.
                codecConfigAnnexB?.let { runCatching { writeFrame(it) } }
                requestSyncFrame()
            } catch (_: java.net.SocketTimeoutException) {
                // No laptop yet (still granting consent / connecting) — loop.
            } catch (e: Exception) {
                Log.w(TAG, "mirror: accept failed: ${e.message}")
                break
            }
        }
    }

    /** Seal one Annex-B access unit and write it to the laptop over TCP. */
    private fun sendAccessUnit(data: ByteArray) {
        if (!clientConnected) return // laptop not connected yet — drop until it is
        writeFrame(data)
    }

    /** Serialised: the encoder loop and the accept coroutine both write here,
     *  and the sealer's nonce counter must stay monotone (single writer). */
    private fun writeFrame(data: ByteArray) = synchronized(writeLock) {
        val s = tcpSealer ?: return
        val out = videoOut ?: return
        try {
            // One sealed, length-prefixed message per AU. TCP is ordered +
            // reliable, so the laptop just concatenates — no loss, no freezes.
            val sealed = s.sealAccessUnit(data)
            // Time only the network write: with a small send buffer it BLOCKS
            // when the link is saturated, so the blocked duration is our
            // congestion signal for adaptive bitrate.
            val t0 = System.nanoTime()
            writeStartedNs = t0
            out.write(sealed)
            out.flush()
            writeStartedNs = 0L
            val now = System.nanoTime()
            adaptBlockedNs += now - t0
            maybeAdaptBitrate(now)
        } catch (e: Exception) {
            writeStartedNs = 0L
            Log.w(TAG, "mirror: video write failed: ${e.message}")
            running = false
        }
    }

    /** Java sockets have no write timeout, so a laptop that stops draining
     *  leaves the sender parked in sendto() forever — the session never ends,
     *  it just freezes, and any teardown that touches the stream deadlocks
     *  behind it. This watchdog turns that into a clean disconnect: if one
     *  write has been in flight longer than [WRITE_STALL_LIMIT_NS], close the
     *  raw socket, which makes the blocked write throw. Adaptive bitrate still
     *  owns the ordinary case (short blocks = congestion); this only catches
     *  the pathological one. */
    private fun startWriteWatchdog() = scope.launch(Dispatchers.IO) {
        while (isActive && running) {
            delay(1_000)
            val started = writeStartedNs
            if (started != 0L && System.nanoTime() - started > WRITE_STALL_LIMIT_NS) {
                Log.w(TAG, "mirror: laptop stopped draining video — dropping the session")
                running = false
                try { videoClient?.close() } catch (_: Throwable) {}
                return@launch
            }
        }
    }

    /** Adaptive bitrate (congestion control), the scrcpy trick for a
     *  freeze-free stream: once per window, look at how much of the wall time
     *  was spent BLOCKED in socket writes. High blocked-fraction = the Wi-Fi
     *  link can't carry the current bitrate → drop it (frames shrink, the link
     *  keeps up, no buffer-then-freeze). Low = headroom → ramp back up toward
     *  the ceiling. Net effect: quality flexes, frame rate stays smooth.
     *  Called under [writeLock]. */
    private fun maybeAdaptBitrate(now: Long) {
        if (adaptWindowStartNs == 0L) { adaptWindowStartNs = now; return }
        val elapsed = now - adaptWindowStartNs
        if (elapsed < ADAPT_WINDOW_NS) return
        val blockedFrac = adaptBlockedNs.toDouble() / elapsed
        adaptWindowStartNs = now
        adaptBlockedNs = 0
        val next = when {
            blockedFrac > ADAPT_BLOCKED_HI -> (currentBitrate * ADAPT_DOWN).toInt()
            blockedFrac < ADAPT_BLOCKED_LO -> (currentBitrate * ADAPT_UP).toInt()
            else -> return
        }.coerceIn(ADAPT_MIN_BITRATE, bitrate)
        if (next != currentBitrate) {
            currentBitrate = next
            try {
                codec?.setParameters(Bundle().apply {
                    putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, next)
                })
            } catch (_: Throwable) {}
        }
    }

    private fun requestSyncFrame() {
        try {
            codec?.setParameters(Bundle().apply {
                putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0)
            })
        } catch (_: Throwable) {}
    }

    private fun releaseMirrorPipeline() {
        try { if (screenWakeLock?.isHeld == true) screenWakeLock?.release() } catch (_: Throwable) {}
        screenWakeLock = null
        try { virtualDisplay?.release() } catch (_: Throwable) {}
        virtualDisplay = null
        try { inputSurface?.release() } catch (_: Throwable) {}
        inputSurface = null
        try { codec?.stop() } catch (_: Throwable) {}
        try { codec?.release() } catch (_: Throwable) {}
        codec = null
        codecConfigAnnexB = null
        try { projection?.stop() } catch (_: Throwable) {}
        projection = null
        clientConnected = false
        // Drop the buffered stream WITHOUT closing it, and close the raw socket
        // first. Closing the wrapper would flush, and flush takes the stream's
        // monitor — which the sender coroutine holds for as long as it sits in
        // sendto(). When the laptop stops draining the socket that is forever,
        // so tearing down here parked the MAIN thread and Android killed us
        // with "Vortex isn't responding" (ANR: onDestroy → FilterOutputStream
        // .close → BufferedOutputStream.flush, blocked on the send thread).
        // Socket.close() makes that blocked write throw instead, and the
        // buffered wrapper owns no OS resource of its own, so letting it go is
        // enough.
        videoOut = null
        try { videoClient?.close() } catch (_: Throwable) {}
        videoClient = null
        try { videoServer?.close() } catch (_: Throwable) {}
        videoServer = null
        tcpSealer = null
    }

    // ── H264 helpers (ported verbatim) ──────────────────────────────────────

    private fun normalizeAccessUnit(accessUnit: ByteArray, isKeyframe: Boolean): ByteArray {
        val annexB = avccToAnnexB(accessUnit)
        val config = codecConfigAnnexB
        return if (isKeyframe && config != null && config.isNotEmpty()) config + annexB else annexB
    }

    private fun buildCodecConfigAnnexB(csd0: ByteArray?, csd1: ByteArray?): ByteArray {
        val out = ArrayList<Byte>()
        appendAnnexBUnit(out, csd0)
        appendAnnexBUnit(out, csd1)
        return out.toByteArray()
    }

    private fun appendAnnexBUnit(out: MutableList<Byte>, bytes: ByteArray?) {
        if (bytes == null || bytes.isEmpty()) return
        if (looksLikeAnnexB(bytes)) { out.addAll(bytes.asList()); return }
        out.addAll(byteArrayOf(0, 0, 0, 1).asList())
        out.addAll(bytes.asList())
    }

    private fun avccToAnnexB(input: ByteArray): ByteArray {
        if (input.isEmpty() || looksLikeAnnexB(input)) return input
        val out = ArrayList<Byte>(input.size + 32)
        var offset = 0
        while (offset + 4 <= input.size) {
            val nalSize =
                ((input[offset].toInt() and 0xFF) shl 24) or
                    ((input[offset + 1].toInt() and 0xFF) shl 16) or
                    ((input[offset + 2].toInt() and 0xFF) shl 8) or
                    (input[offset + 3].toInt() and 0xFF)
            offset += 4
            if (nalSize <= 0 || offset + nalSize > input.size) return input
            out.addAll(byteArrayOf(0, 0, 0, 1).asList())
            for (i in offset until offset + nalSize) out.add(input[i])
            offset += nalSize
        }
        return if (out.isEmpty()) input else out.toByteArray()
    }

    private fun looksLikeAnnexB(b: ByteArray): Boolean {
        if (b.size < 4) return false
        return (b[0].toInt() == 0 && b[1].toInt() == 0 && b[2].toInt() == 0 && b[3].toInt() == 1) ||
            (b[0].toInt() == 0 && b[1].toInt() == 0 && b[2].toInt() == 1)
    }

    // ── Notification ────────────────────────────────────────────────────────

    private fun buildNotification(text: String): Notification {
        val stop = PendingIntent.getService(
            this, 0,
            Intent(this, ScreenMirrorService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Vortex screen mirroring")
            .setContentText(text)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setOnlyAlertOnce(true)
            .setOngoing(true)
            .addAction(android.R.drawable.ic_media_pause, "Stop", stop)
            .build()
    }

    // Log-only: we deliberately DON'T re-post the notification with live status
    // (frame counts etc.) — the constant "frames=N" updates were noisy in the
    // status bar. The foreground notification stays as its initial quiet text.
    private fun updateStatus(text: String) {
        Log.d(TAG, text)
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        getSystemService(NotificationManager::class.java)?.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "Vortex Mirror", NotificationManager.IMPORTANCE_MIN),
        )
    }

    companion object {
        private const val TAG = "VortexMirror"
        private const val CHANNEL_ID = "vortex_mirror"
        private const val NOTIFICATION_ID = 7301

        /** Fixed TCP port the phone serves video on; the laptop connects out to
         *  it. MUST match the Rust `mirror_tcp::VIDEO_PORT`. */
        private const val MIRROR_VIDEO_PORT = 51822

        /** accept() poll timeout so the coroutine can re-check running/cancel. */
        private const val VIDEO_ACCEPT_TIMEOUT_MS = 1_000

        /** Small send buffer → when the link congests, the blocking write()
         *  back-pressures the encoder (it drops to the deliverable frame rate)
         *  instead of bloating a big buffer that later bursts out as a freeze.
         *  scrcpy's trick: trade fluidity for steadiness under a weak link. */
        private const val VIDEO_SEND_BUFFER = 64 * 1024

        // Adaptive-bitrate (congestion-control) tuning.
        private const val ADAPT_MIN_BITRATE = 800_000     // floor (keeps it legible)
        private const val ADAPT_START_FRAC = 0.6          // start at 60% of ceiling, ramp up
        private const val ADAPT_WINDOW_NS = 700_000_000L  // re-evaluate ~1.4×/sec
        private const val ADAPT_BLOCKED_HI = 0.20         // >20% time blocked → link saturated
        private const val ADAPT_BLOCKED_LO = 0.05         // <5% blocked → headroom
        private const val ADAPT_DOWN = 0.75               // back off fast on congestion
        private const val ADAPT_UP = 1.12                 // ramp up gently

        /** One socket write stuck this long means the laptop is not draining at
         *  all — congestion never looks like this, and no adaptive bitrate can
         *  fix it. Long enough that a bad Wi-Fi moment is not mistaken for it. */
        private const val WRITE_STALL_LIMIT_NS = 6_000_000_000L

        const val ACTION_START = "com.vortex.a3.action.MIRROR_START"
        const val ACTION_STOP = "com.vortex.a3.action.MIRROR_STOP"
        const val ACTION_REQUEST_KEYFRAME = "com.vortex.a3.action.MIRROR_KEYFRAME"
        const val EXTRA_RESULT_CODE = "result_code"
        const val EXTRA_RESULT_DATA = "result_data"
        const val EXTRA_IP = "ip"
        const val EXTRA_UDP_PORT = "udp_port"
        const val EXTRA_WIDTH = "width"
        const val EXTRA_HEIGHT = "height"
        const val EXTRA_FPS = "fps"
        const val EXTRA_BITRATE = "bitrate"
        const val EXTRA_KEY = "media_key"

        /** Start streaming. Caller (VortexStack) supplies the consent token +
         *  the mirror-session params (UDP target, encoder config, media key). */
        fun start(
            context: Context,
            resultCode: Int,
            resultData: Intent,
            ip: String,
            udpPort: Int,
            width: Int,
            height: Int,
            fps: Int,
            bitrate: Int,
            mediaKey: ByteArray,
        ) {
            val intent = Intent(context, ScreenMirrorService::class.java).apply {
                action = ACTION_START
                putExtra(EXTRA_RESULT_CODE, resultCode)
                putExtra(EXTRA_RESULT_DATA, resultData)
                putExtra(EXTRA_IP, ip)
                putExtra(EXTRA_UDP_PORT, udpPort)
                putExtra(EXTRA_WIDTH, width)
                putExtra(EXTRA_HEIGHT, height)
                putExtra(EXTRA_FPS, fps)
                putExtra(EXTRA_BITRATE, bitrate)
                putExtra(EXTRA_KEY, mediaKey)
            }
            ContextCompat.startForegroundService(context, intent)
        }

        fun requestKeyframe(context: Context) {
            context.startService(
                Intent(context, ScreenMirrorService::class.java)
                    .setAction(ACTION_REQUEST_KEYFRAME),
            )
        }

        fun stop(context: Context) {
            context.startService(
                Intent(context, ScreenMirrorService::class.java).setAction(ACTION_STOP),
            )
        }
    }
}

private fun ByteBuffer.toByteArray(): ByteArray {
    val dup = duplicate()
    val bytes = ByteArray(dup.remaining())
    dup.get(bytes)
    return bytes
}
