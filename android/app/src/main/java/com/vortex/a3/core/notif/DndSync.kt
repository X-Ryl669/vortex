package com.vortex.a3.core.notif

import android.service.notification.NotificationListenerService
import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/**
 * Do Not Disturb, shared with the laptop.
 *
 * Reading and setting the interruption filter is something a
 * [NotificationListenerService] may do with the access it already holds, so
 * this feature adds no permission prompt of its own. Measured on this phone
 * before writing any of it: MIUI maps its own DND onto the AOSP filter
 * correctly (`zen_mode=1` reads back as `INTERRUPTION_FILTER_PRIORITY`), which
 * is the thing that could have sunk the idea.
 *
 * The wire contract lives in `AppState.dnd` — last-writer-wins on
 * `dndChangedAt`, and ONLY an explicit toggle advances the clock. A heartbeat
 * that merely observed the current state would fight the other device: turn
 * DND off after a meeting and a stale beat from the laptop turns it back on.
 */
object DndSync {
    private const val TAG = "VortexDnd"

    /** Our view of the shared setting and when it was last explicitly set. */
    private val on = AtomicBoolean(false)
    private val changedAt = AtomicLong(0L)

    /** True when the filter means "hold my notifications". */
    private fun filterIsDnd(filter: Int): Boolean =
        filter == NotificationListenerService.INTERRUPTION_FILTER_PRIORITY ||
            filter == NotificationListenerService.INTERRUPTION_FILTER_NONE ||
            filter == NotificationListenerService.INTERRUPTION_FILTER_ALARMS

    /** Persisted so a process restart RESUMES the shared opinion instead of
     *  starting at "no opinion", which any peer stamp would beat. Same reason
     *  and same shape as SmartSwitchSetting. */
    private const val PREFS = "vortex_ui_settings"
    private const val KEY_ON = "dnd_on"
    private const val KEY_AT = "dnd_changed_at"
    private var prefs: android.content.SharedPreferences? = null

    fun init(ctx: android.content.Context) {
        if (prefs != null) return
        val p = ctx.applicationContext.getSharedPreferences(PREFS, android.content.Context.MODE_PRIVATE)
        prefs = p
        on.set(p.getBoolean(KEY_ON, false))
        changedAt.set(p.getLong(KEY_AT, 0L))
        Log.i(TAG, "restored dnd=${on.get()} at=${changedAt.get()}")
    }

    private fun save() {
        prefs?.edit()?.putBoolean(KEY_ON, on.get())?.putLong(KEY_AT, changedAt.get())?.apply()
    }

    fun state(): Pair<Boolean, Long> = on.get() to changedAt.get()

    /** Seed from the phone's current filter at listener-connect time, without
     *  claiming authorship: `changedAt` stays where it is, so this never wins
     *  an LWW comparison against a real toggle on the laptop. */
    fun seed(filter: Int) {
        val real = filterIsDnd(filter)
        if (changedAt.get() == 0L) {
            // No opinion of our own yet — take the phone's current state as
            // the starting point, still without claiming authorship.
            on.set(real)
            Log.i(TAG, "seeded from the phone's filter: dnd=$real")
            return
        }
        // We DO have a saved opinion. If the phone no longer matches it, it
        // was changed while we were not listening — a local edge, and it needs
        // a fresh stamp to win, exactly as on the laptop.
        if (real != on.get()) {
            Log.i(TAG, "phone changed while unbound: dnd=$real")
            confirmLocalChange()
        }
    }

    /** The user changed DND ON THIS PHONE — stamp it so the laptop adopts it.
     *
     *  Value-based, not time-based. The previous version ignored filter
     *  changes for a fixed 2s after we set the filter ourselves, and MIUI
     *  defeated that twice: it delivers the callback later than the window,
     *  and it passes through an INTERMEDIATE filter on the way (PRIORITY →
     *  ALARMS → ALL) whose classification is the opposite of the destination.
     *  An intermediate arriving after the window was stamped as a user toggle
     *  and sent back, and the laptop dutifully reverted.
     *
     *  So: a filter that agrees with what we already believe is never news,
     *  whatever caused it. A filter that disagrees is only believed if it is
     *  STILL disagreeing a moment later — which a pass-through value never is,
     *  and a real toggle always is. */
    fun noteLocalChange(filter: Int) {
        val dnd = filterIsDnd(filter)
        if (dnd == on.get()) return
        settleHandler.removeCallbacksAndMessages(null)
        settleHandler.postDelayed({ confirmLocalChange() }, settleMs())
    }

    private val settleHandler = android.os.Handler(android.os.Looper.getMainLooper())

    /** How long to let the filter settle before believing it.
     *
     *  The long wait only ever existed for ONE case: the filter passing through
     *  an intermediate value on its way to the one WE just asked for. Outside
     *  that window nothing is passing through — the callback carries the user's
     *  own toggle — and making every toggle wait for a hazard that is not
     *  present is what made this sync feel slow. So the wait follows the
     *  hazard: [SETTLE_AFTER_SELF_MS] while our own write could still be
     *  landing, [SETTLE_MS] otherwise.
     *
     *  The value check in [confirmLocalChange] is still the real guard; this
     *  only decides when to run it. */
    private fun settleMs(): Long {
        val since = android.os.SystemClock.elapsedRealtime() - lastSelfApplyAtMs.get()
        return if (since < SELF_APPLY_WINDOW_MS) SETTLE_AFTER_SELF_MS else SETTLE_MS
    }

    private const val SETTLE_MS = 200L
    private const val SETTLE_AFTER_SELF_MS = 900L
    private const val SELF_APPLY_WINDOW_MS = 3_000L

    /** When we last asked the platform to change the filter ourselves. */
    private val lastSelfApplyAtMs = AtomicLong(0L)

    /** Re-read the filter after it has had time to settle, and only then treat
     *  a disagreement as the user's doing. */
    private fun confirmLocalChange() {
        val svc = com.vortex.a3.core.media.MediaNotificationListenerService.instanceOrNull()
            ?: return
        val dnd = try {
            filterIsDnd(svc.currentInterruptionFilter)
        } catch (t: Throwable) {
            Log.w(TAG, "could not re-read the filter: ${t.message}")
            return
        }
        if (dnd == on.get()) return // it was a pass-through, not a change
        on.set(dnd)
        // Monotonic, for the same reason the laptop's stamp is: a toggle made
        // just after adopting the peer's value must not carry a stamp the peer
        // discards as not-newer.
        val at = maxOf(System.currentTimeMillis() / 1000L, changedAt.get() + 1)
        changedAt.set(at)
        save()
        Log.i(TAG, "changed here -> dnd=$dnd at=$at; propagating")
        // BLE first: one STATE frame on the link that is already up reaches the
        // laptop in ~200 ms and costs no radio wake of its own. The LAN nudge
        // behind it is the backstop for a BLE link that is down or wedged — it
        // only re-announces mDNS, so it costs nothing when BLE already won.
        com.vortex.a3.service.VortexService.nudgeAppState()
        com.vortex.a3.service.VortexService.liveLan?.nudge()
    }

    /** Apply the laptop's setting if its toggle is newer than ours. */
    fun applyPeer(service: NotificationListenerService?, peerOn: Boolean, peerAt: Long) {
        if (peerAt == 0L || peerAt <= changedAt.get()) return
        // The service FIRST. Recording the peer's value and stamp and then
        // finding no listener to apply them with left the phone advertising a
        // state it was not in, and — because the stamp had advanced — nothing
        // ever re-applied it. MIUI unbinds the listener freely, so this is not
        // a rare path. Leaving `changedAt` alone means the next snapshot tries
        // again.
        val svc = service ?: run {
            Log.w(TAG, "listener not bound — cannot apply dnd=$peerOn yet")
            return
        }
        changedAt.set(peerAt)
        if (on.getAndSet(peerOn) == peerOn) {
            save()
            return
        }
        save()
        val want = if (peerOn) {
            NotificationListenerService.INTERRUPTION_FILTER_PRIORITY
        } else {
            NotificationListenerService.INTERRUPTION_FILTER_ALL
        }
        try {
            // Stamp BEFORE the write: the callback for our own change can land
            // while `requestInterruptionFilter` is still returning, and it must
            // find the longer settle window already open.
            lastSelfApplyAtMs.set(android.os.SystemClock.elapsedRealtime())
            svc.requestInterruptionFilter(want)
            Log.i(TAG, "adopted the laptop's setting: dnd=$peerOn")
        } catch (t: Throwable) {
            // MIUI can refuse this even with listener access granted. Nothing
            // to retry — the user's own toggle still works and still syncs the
            // other way; only this direction is lost.
            Log.w(TAG, "could not set the interruption filter: ${t.message}")
        }
    }
}
