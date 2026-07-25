package com.vortex.a3.service

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.ComponentName
import android.content.Context
import android.graphics.Path
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.util.DisplayMetrics
import android.util.Log
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import kotlin.math.abs

/**
 * Laptop → phone input injector. The mirror window on the laptop captures the
 * user's mouse/touchpad/keyboard and sends 5-byte `[type][x:u16][y:u16]` packets
 * over the sealed control plane; [MirrorSession] hands each to [onPacket]. We
 * replay them as touch gestures via the AccessibilityService gesture API — the
 * only no-root, no-adb way an app can drive other apps' UI (phone-mirroring
 * style: one accessibility toggle, no cable, no developer mode).
 *
 * Coordinates arrive normalized to 0..65535 of the mirrored frame; we scale to
 * the phone's real display pixels (the capture is the full display).
 *
 * Robustness model — the framework BLOCKS all `dispatchGesture` calls while a
 * `willContinue` gesture is open, so leaving one open (a paused drag, or a lost
 * UP when the mouse releases off-window) freezes all control. We avoid that:
 *   - A stationary press (no movement) is committed only on UP as ONE gesture:
 *     a quick tap, or a long-press if the button was held. Never opens a chain.
 *   - A moving press (drag/swipe/scroll) uses a live `continueStroke` chain, but
 *     the moment motion stops it CLOSES within [IDLE_CLOSE_MS] instead of
 *     hanging open — so a lost UP self-heals fast and taps are never blocked.
 *   - A [watchdog] is the final backstop if input stops mid-gesture.
 * Nav packets (BACK/HOME/RECENTS) map to global actions and ignore x/y.
 *
 * All gesture state is touched only on the main thread (packets are re-posted
 * there in [onPacket]).
 */
class VortexInputService : AccessibilityService() {

    private val main = Handler(Looper.getMainLooper())

    // Real display size in px, cached on connect (gesture coords are absolute
    // display pixels including system bars — getRealMetrics gives exactly that).
    private var dispW = 0f
    private var dispH = 0f

    // Browsing-handoff state: the last URL advertised to the laptop (dedup), its
    // source package (for the heartbeat re-assert), and when we last read the
    // address bar (debounce the content-changed stream).
    private var lastHandoffUrl: String? = null
    private var lastHandoffPkg: String? = null
    private var lastHandoffAt = 0L

    // Press tracking (decides tap vs long-press vs drag).
    private var downActive = false
    private var movedSinceDown = false
    private var downX = 0f
    private var downY = 0f
    private var downAtMs = 0L

    // Live-drag chain state.
    private var lastStroke: GestureDescription.StrokeDescription? = null
    private var gestureInFlight = false
    private var touchActive = false // the synthetic finger is down (a drag chain)
    private var pendingLift = false
    private var curX = 0f
    private var curY = 0f
    private var tgtX = 0f
    private var tgtY = 0f

    override fun onServiceConnected() {
        super.onServiceConnected()
        instance = this
        cacheDisplaySize()
        main.postDelayed(handoffHeartbeat, HANDOFF_HEARTBEAT_MS)
        Log.i(TAG, "input service connected (${dispW.toInt()}x${dispH.toInt()})")
    }

    override fun onUnbind(intent: android.content.Intent?): Boolean {
        if (instance === this) instance = null
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        main.removeCallbacks(watchdog)
        main.removeCallbacks(idleClose)
        main.removeCallbacks(handoffHeartbeat)
        if (instance === this) instance = null
        super.onDestroy()
    }

    // Re-assert the active page to the laptop every ~25s. The laptop sweeps a
    // handoff pill it hasn't heard about for 90s (force-stop safety net), but a
    // user sitting on one page emits NO nav events, so without this heartbeat
    // the pill vanishes mid-read. Mirrors the laptop's live-activity heartbeat
    // model; no-op (and harmless) while no browser page is foreground.
    private val handoffHeartbeat = object : Runnable {
        override fun run() {
            val url = lastHandoffUrl
            if (url != null) {
                VortexService.handoffBus.tryEmit(
                    com.vortex.a3.core.handoff.HandoffEvent(
                        url = url, appId = lastHandoffPkg ?: "", openNow = false,
                    ),
                )
            }
            main.postDelayed(this, HANDOFF_HEARTBEAT_MS)
        }
    }

    // Browsing HANDOFF (seamless-continuity): while a known browser is foreground,
    // read its address bar and advertise the page to the laptop (a "continue
    // from phone" pill). We never CONSUME events (return without handling). The
    // laptop opens the URL only on the user's click, so this read is advisory.
    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        try {
            handleHandoff(event)
        } catch (_: Throwable) {
            // never let an accessibility read crash the (control) service
        }
    }

    override fun onInterrupt() {}

    private fun handleHandoff(event: AccessibilityEvent?) {
        event ?: return
        // Debounce the very noisy content/state stream.
        val now = android.os.SystemClock.uptimeMillis()
        if (now - lastHandoffAt < HANDOFF_THROTTLE_MS) return
        lastHandoffAt = now

        // Use the ACTUAL foreground window's package (not the event source) so a
        // transient overlay/keyboard/system-ui event while the user is in Chrome
        // doesn't look like "they left the browser" and clear the pill.
        val root = rootInActiveWindow ?: return
        val pkg = root.packageName?.toString() ?: return
        val urlBarId = BROWSER_URL_BARS[pkg]
        if (urlBarId == null) {
            // Foreground is a non-browser app → stop offering the page.
            if (lastHandoffUrl != null) {
                lastHandoffUrl = null
                lastHandoffPkg = null
                VortexService.handoffBus.tryEmit(
                    com.vortex.a3.core.handoff.HandoffEvent(url = "", openNow = false),
                )
            }
            return
        }
        val raw = try {
            root.findAccessibilityNodeInfosByViewId(urlBarId)
                ?.firstOrNull()?.text?.toString()?.trim()
        } catch (_: Throwable) {
            null
        }
        val url = normalizeUrl(raw) ?: return
        if (url == lastHandoffUrl) return
        lastHandoffUrl = url
        lastHandoffPkg = pkg
        VortexService.handoffBus.tryEmit(
            com.vortex.a3.core.handoff.HandoffEvent(url = url, appId = pkg, openNow = false),
        )
    }

    /** The omnibox text may be a full URL, a bare domain ("example.com"), or a
     *  search term. Return a usable http(s) URL or null (don't hand off a search). */
    private fun normalizeUrl(raw: String?): String? {
        if (raw.isNullOrBlank()) return null
        val s = raw.trim()
        if (s.startsWith("http://") || s.startsWith("https://")) return s
        // A bare domain (has a dot, no whitespace) → assume https.
        if (!s.any { it.isWhitespace() } && s.contains('.')) return "https://$s"
        return null
    }

    private fun cacheDisplaySize() {
        try {
            val wm = getSystemService(WindowManager::class.java)
            val dm = DisplayMetrics()
            @Suppress("DEPRECATION")
            wm.defaultDisplay.getRealMetrics(dm)
            dispW = dm.widthPixels.toFloat()
            dispH = dm.heightPixels.toFloat()
        } catch (e: Exception) {
            Log.w(TAG, "display size: ${e.message}")
        }
    }

    // Fires if input stops mid-gesture (lost UP / cancelled by a physical touch).
    private val watchdog = Runnable {
        if (touchActive || gestureInFlight || downActive) {
            Log.i(TAG, "watchdog: clearing stuck input state")
            forceReset()
        }
    }

    // Closes a live drag once motion has paused, so the chain is never left open
    // (which would block all further injection).
    private val idleClose = Runnable {
        if (touchActive && !pendingLift) {
            pendingLift = true
            if (!gestureInFlight) tick()
        }
    }

    /** Called from the mirror network thread with a 5-byte input packet. */
    fun onPacket(pkt: ByteArray) {
        if (pkt.size != 5) return
        val type = pkt[0].toInt() and 0xFF
        val nx = ((pkt[1].toInt() and 0xFF) shl 8) or (pkt[2].toInt() and 0xFF)
        val ny = ((pkt[3].toInt() and 0xFF) shl 8) or (pkt[4].toInt() and 0xFF)
        main.post {
            main.removeCallbacks(watchdog)
            main.postDelayed(watchdog, WATCHDOG_MS)
            dispatch(type, nx, ny)
        }
    }

    private fun dispatch(type: Int, nx: Int, ny: Int) {
        when (type) {
            T_DOWN -> onDown(toX(nx), toY(ny))
            T_MOVE -> onMove(toX(nx), toY(ny))
            T_UP -> onUp(toX(nx), toY(ny))
            T_BACK -> performGlobalAction(GLOBAL_ACTION_BACK)
            T_HOME -> performGlobalAction(GLOBAL_ACTION_HOME)
            T_RECENTS -> performGlobalAction(GLOBAL_ACTION_RECENTS)
        }
    }

    private fun onDown(x: Float, y: Float) {
        downActive = true
        movedSinceDown = false
        downX = x; downY = y
        downAtMs = SystemClock.uptimeMillis()
        curX = x; curY = y
        tgtX = x; tgtY = y
    }

    private fun onMove(x: Float, y: Float) {
        tgtX = x; tgtY = y
        movedSinceDown = true
        when {
            // Start a live drag, or resume one that idle-closed while the button
            // was still held (re-press from where the finger lifted).
            !touchActive && downActive -> beginDrag()
            touchActive -> { main.removeCallbacks(idleClose); tick() }
        }
    }

    private fun onUp(x: Float, y: Float) {
        if (touchActive) {
            tgtX = x; tgtY = y
            pendingLift = true
            main.removeCallbacks(idleClose)
            tick() // move to target, then release
        } else if (downActive && !movedSinceDown) {
            // A stationary press → tap, or long-press if the button was held.
            val held = SystemClock.uptimeMillis() - downAtMs
            if (held >= LONG_PRESS_MS) longPress(downX, downY, held) else quickTap(downX, downY)
        }
        downActive = false
    }

    private fun beginDrag() {
        touchActive = true
        pendingLift = false
        lastStroke = null // fresh press at the current finger point
        tick()
    }

    /**
     * The ONLY place a drag segment is dispatched. Guarded by [gestureInFlight]
     * so there is never more than one gesture pending. Drives cur→tgt and
     * re-arms from the completion callback; when it catches up it schedules
     * [idleClose] rather than leaving the gesture open.
     */
    private fun tick() {
        if (!touchActive || gestureInFlight) return
        val needMove = abs(tgtX - curX) >= 0.5f || abs(tgtY - curY) >= 0.5f
        if (!needMove && !pendingLift) {
            main.removeCallbacks(idleClose)
            main.postDelayed(idleClose, IDLE_CLOSE_MS) // caught up — close soon
            return
        }

        val willContinue = !pendingLift
        val nx = if (needMove) tgtX else curX + 1f // non-empty contour on lift
        val ny = if (needMove) tgtY else curY
        val p = Path()
        p.moveTo(curX, curY)
        p.lineTo(nx, ny)
        val stroke = lastStroke?.let { prev ->
            try {
                prev.continueStroke(p, 0, SEG_MS, willContinue)
            } catch (e: Exception) {
                null
            }
        } ?: GestureDescription.StrokeDescription(p, 0, SEG_MS, willContinue)
        lastStroke = if (willContinue) stroke else null

        gestureInFlight = true
        val ok = dispatchGesture(
            GestureDescription.Builder().addStroke(stroke).build(),
            object : GestureResultCallback() {
                override fun onCompleted(d: GestureDescription?) {
                    gestureInFlight = false
                    curX = nx; curY = ny
                    if (!willContinue) endDrag() else tick()
                }

                override fun onCancelled(d: GestureDescription?) {
                    gestureInFlight = false
                    curX = nx; curY = ny
                    lastStroke = null
                    endDrag() // a cancelled stroke is already released by the OS
                }
            },
            main,
        )
        if (!ok) {
            gestureInFlight = false
            lastStroke = null
            endDrag()
        }
    }

    private fun endDrag() {
        touchActive = false
        pendingLift = false
        lastStroke = null
        main.removeCallbacks(idleClose)
    }

    private fun quickTap(x: Float, y: Float, retry: Boolean = true) {
        if (!dispatchStationary(x, y, TAP_MS) && retry) {
            // A dangling gesture is blocking dispatch — clear state and retry once.
            forceReset()
            main.postDelayed({ quickTap(x, y, retry = false) }, RETRY_MS)
        }
    }

    private fun longPress(x: Float, y: Float, heldMs: Long) {
        dispatchStationary(x, y, heldMs.coerceIn(LONG_PRESS_MS, MAX_PRESS_MS))
    }

    /** Dispatch a one-shot stationary press of [durMs] (tap / long-press).
     *  Returns false if the framework rejected it (a gesture is in progress). */
    private fun dispatchStationary(x: Float, y: Float, durMs: Long): Boolean {
        val p = Path()
        p.moveTo(x, y)
        p.lineTo(x + 1f, y)
        return try {
            val stroke = GestureDescription.StrokeDescription(p, 0, durMs, false)
            dispatchGesture(GestureDescription.Builder().addStroke(stroke).build(), null, null)
        } catch (e: Exception) {
            Log.w(TAG, "press: ${e.message}")
            true
        }
    }

    /** Hard-reset to idle so the next laptop action starts clean. */
    private fun forceReset() {
        gestureInFlight = false
        touchActive = false
        pendingLift = false
        downActive = false
        movedSinceDown = false
        lastStroke = null
        main.removeCallbacks(idleClose)
    }

    private fun toX(nx: Int) = if (dispW > 0f) nx / 65535f * dispW else 0f
    private fun toY(ny: Int) = if (dispH > 0f) ny / 65535f * dispH else 0f

    companion object {
        private const val TAG = "VortexInput"

        /** Min gap between address-bar reads — the content-changed stream is very
         *  noisy (scroll, animations); we only need the URL when it changes. */
        private const val HANDOFF_THROTTLE_MS = 1200L

        /** Re-assert the active page this often so the laptop's 90s staleness
         *  sweep never drops the pill while the user lingers on one page. */
        private const val HANDOFF_HEARTBEAT_MS = 25_000L

        /** Browsers we can read the address bar of → its URL-bar view id. */
        private val BROWSER_URL_BARS = mapOf(
            "com.android.chrome" to "com.android.chrome:id/url_bar",
            "com.chrome.beta" to "com.chrome.beta:id/url_bar",
            "com.chrome.dev" to "com.chrome.dev:id/url_bar",
            "com.microsoft.emmx" to "com.microsoft.emmx:id/url_bar",
            "com.brave.browser" to "com.brave.browser:id/url_bar",
            "org.mozilla.firefox" to "org.mozilla.firefox:id/mozac_browser_toolbar_url_view",
        )

        /** Live instance while the service is connected (same process as the
         *  app, so a direct singleton is the simplest hand-off). */
        @Volatile
        var instance: VortexInputService? = null

        // Wire protocol (mirror l3 mirror.rs `input_proto`).
        const val T_DOWN = 0
        const val T_MOVE = 1
        const val T_UP = 2
        const val T_BACK = 10
        const val T_HOME = 11
        const val T_RECENTS = 12

        // Drag segment length: short so the finger tracks the cursor tightly.
        private const val SEG_MS = 5L
        // Tap press duration — near-instant.
        private const val TAP_MS = 1L
        // Held-press threshold for a long-press, and its dispatch cap.
        private const val LONG_PRESS_MS = 400L
        private const val MAX_PRESS_MS = 1_500L
        // Pause after motion stops before the drag finger lifts (keeps the
        // framework from staying blocked on an open gesture).
        private const val IDLE_CLOSE_MS = 150L
        // Idle time with a touch still down before we assume a lost UP and reset.
        private const val WATCHDOG_MS = 700L
        // Backoff before retrying a tap that couldn't be dispatched.
        private const val RETRY_MS = 12L

        /** Whether the user has enabled our accessibility service (canonical
         *  check via Secure settings — valid even before the service binds). */
        fun isEnabled(ctx: Context): Boolean {
            val flat = Settings.Secure.getString(
                ctx.contentResolver,
                Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
            ) ?: return false
            val cn = ComponentName(ctx, VortexInputService::class.java).flattenToString()
            return flat.split(':').any { it.equals(cn, ignoreCase = true) }
        }
    }
}
