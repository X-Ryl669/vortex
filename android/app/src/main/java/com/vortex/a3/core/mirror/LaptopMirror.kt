package com.vortex.a3.core.mirror

import android.content.Context
import android.content.Intent
import android.util.Log
import com.vortex.a3.ui.LaptopMirrorActivity

/**
 * Phone-side coordinator for the LAPTOP→phone screen mirror (view-only).
 *
 * Flow:
 *  1. User taps "view laptop screen" → [requestView] flips [requestActive] on and
 *     nudges the heartbeat. The AppState builder ships `laptopMirrorReq = true`.
 *  2. The laptop sees the request, pops its screen-share consent and, on accept,
 *     starts casting + advertises `{ip, port, key}` in ITS AppState.
 *  3. The phone's AppState handler calls [onLaptopOffer], which launches
 *     [LaptopMirrorActivity] once to connect + decode.
 *  4. Closing the viewer calls [onViewerClosed], dropping the request so the
 *     laptop releases its capture.
 *
 * One viewer at a time; all state is process-global (a single user, a single
 * laptop). The 32-byte media key is the laptop→phone key, delivered over the
 * Noise-sealed control channel — never derived or logged here.
 */
object LaptopMirror {
    private const val TAG = "LaptopMirror"

    /** Consecutive offer-less heartbeats before we tear the viewer down (the
     *  laptop sends its AppState every ~1-2s, so ~5 ≈ several seconds of
     *  confirmed "not casting" — well past any start-up race). */
    private const val MISS_LIMIT = 5

    /** True while the user wants to see the laptop screen. Read by the AppState
     *  builder → `laptopMirrorReq`; the laptop casts only while it's set. */
    @Volatile
    var requestActive: Boolean = false
        private set

    /** Set once the viewer is on screen, so a re-sent offer doesn't relaunch it. */
    @Volatile
    private var viewerOpen: Boolean = false

    /** Consecutive heartbeats seen WITHOUT a `laptop_cast` offer while the viewer
     *  is open. We only tear the viewer down after several in a row — a single
     *  transient null (a heartbeat built the instant the cast was (re)starting)
     *  must NOT close it, or it thrashes (close→req off→cast stop→restart→new
     *  key→AEAD mismatch→black). Real "Stop sharing" yields a sustained null. */
    @Volatile
    private var castMisses: Int = 0

    /** Set by VortexStack: ship the local AppState NOW (BLE + LAN) so a request
     *  flip reaches the laptop within ~1s instead of waiting a heartbeat. */
    @Volatile
    var onRequestChanged: (() -> Unit)? = null

    /** Set by the live viewer Activity to finish itself; cleared on close. Lets
     *  the stack tear the viewer down when the LAPTOP stops casting. */
    @Volatile
    var viewerCloser: (() -> Unit)? = null

    /** User tapped "view laptop screen". */
    fun requestView() {
        if (requestActive) return
        requestActive = true
        Log.i(TAG, "view-laptop requested")
        onRequestChanged?.invoke()
    }

    /** The laptop is casting: launch the viewer once. The phone is the video
     *  SERVER (the laptop dials us — only laptop→phone connections survive real
     *  networks), so we just need the port to listen on + the key to open with. */
    fun onLaptopOffer(ctx: Context, port: Int, key: ByteArray) {
        castMisses = 0 // an offer is present → reset the teardown debounce
        if (!requestActive || viewerOpen) return
        if (port == 0 || key.size != 32) {
            Log.w(TAG, "ignoring malformed laptop cast offer")
            return
        }
        viewerOpen = true
        Log.i(TAG, "laptop cast offer → launching viewer (server :$port)")
        val intent = Intent(ctx, LaptopMirrorActivity::class.java).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            putExtra(LaptopMirrorActivity.EXTRA_PORT, port)
            putExtra(LaptopMirrorActivity.EXTRA_KEY, key)
        }
        try {
            ctx.startActivity(intent)
        } catch (t: Throwable) {
            Log.w(TAG, "viewer launch failed: ${t.message}")
            viewerOpen = false
        }
    }

    /** Viewer closed → stop requesting so the laptop releases its capture. */
    fun onViewerClosed(@Suppress("UNUSED_PARAMETER") ctx: Context) {
        viewerOpen = false
        if (!requestActive) return
        requestActive = false
        Log.i(TAG, "viewer closed → cast request cleared")
        onRequestChanged?.invoke()
    }

    /** The LAPTOP stopped casting (its AppState `laptop_cast` cleared — user hit
     *  "Stop sharing" on the laptop, or the capture errored). Tear the viewer
     *  down on this side too so the screens stay in sync. No-op if no viewer. */
    fun onLaptopCastEnded() {
        if (!viewerOpen) return
        // Debounce: only tear down after a SUSTAINED absence of the offer, so a
        // lone transient null doesn't kill a healthy session.
        if (++castMisses < MISS_LIMIT) return
        Log.i(TAG, "laptop stopped casting ($castMisses misses) → closing viewer")
        castMisses = 0
        viewerCloser?.invoke() // Activity.finish() → onViewerClosed() clears state
    }
}
