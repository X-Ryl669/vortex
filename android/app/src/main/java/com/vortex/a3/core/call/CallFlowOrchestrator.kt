package com.vortex.a3.core.call

import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioManager
import android.net.Uri
import android.os.Build
import android.provider.ContactsContract
import android.telecom.TelecomManager
import android.telephony.PhoneStateListener
import android.telephony.TelephonyCallback
import android.telephony.TelephonyManager
import android.util.Log
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/**
 * Call-handoff state machine (Phase 2 in
 * `the earbuds-switch design notes` §8).
 *
 * Listens to [TelephonyManager] and reacts to RINGING / OFFHOOK /
 * IDLE transitions:
 *
 *  - **RINGING** (or first OFFHOOK from Idle) — we want the buds on
 *    the phone *now* so the user can answer. If a hand-off grab was
 *    actually started, arms a 2-second speakerphone fallback so the
 *    caller is audible even if it stalls (busy laptop, network blip,
 *    …). No hand-off (buds already here / BT off) → NO fallback:
 *    forcing speakerphone would hijack a perfectly good route.
 *  - **IDLE** (call ended) — buds go back to the laptop and the
 *    laptop resumes its media. We disengage speakerphone and clear
 *    the in-flight state.
 *
 * The orchestrator is intentionally *not* responsible for sending
 * AUDIO_OP frames itself. Two callbacks plumb that into the rest of
 * the stack — [onCallStart] when ringing/offhook begins and
 * [onCallEnd] when the call wraps up — so this class stays a thin
 * adapter over the platform Telephony APIs.
 */
class CallFlowOrchestrator(
    private val context: Context,
    /** Kick the buds hand-off for a new call. Returns true only if a grab
     *  from the peer actually STARTED — that's what arms the speakerphone
     *  fallback. False (buds already on the phone, BT off, no peer) means
     *  the existing audio route is fine and must not be touched. */
    private val onCallStart: () -> Boolean,
    private val onCallEnd: () -> Unit,
    /** Emits a [CallEvent] on every phase change (ringing → active → ended) so
     *  the stack can mirror the call to the laptop. Defaults to a no-op so the
     *  audio-handoff wiring is unaffected if the mirror isn't wanted. */
    private val onCallEvent: (CallEvent) -> Unit = {},
) {

    /** Identity + caller info for the call currently in flight, so the active
     *  and ended events can reuse what we captured at ringing/offhook. */
    @Volatile private var current: CallEvent? = null

    enum class CallPhase {
        /** No active call. */
        Idle,
        /** Phone is ringing — caller hasn't been answered yet. */
        Ringing,
        /** Call is connected (or outgoing dialled). */
        Active,
        /** Call ended; transient state while we hand the buds back to
         *  the laptop and resume its media. Cleared to Idle after the
         *  switch flow returns. */
        EndingHandoff,
    }

    private val tag = "CallFlow"
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    private val phaseFlow = MutableStateFlow(CallPhase.Idle)

    /** Observable state for callers (UI, VortexService heartbeat). */
    val phase: StateFlow<CallPhase> = phaseFlow.asStateFlow()

    /** True once an in-flight 2-second speakerphone fallback timer is
     *  armed — cancelled when the buds connect, fires
     *  [enableSpeakerphone] when the timer expires. */
    @Volatile private var speakerphoneFallbackJob: Job? = null

    /** Whichever Telephony listener we registered (the API differs
     *  before/after Android 12). Keep so we can unregister cleanly. */
    private var registeredCallback: TelephonyCallback? = null
    private var registeredListener: PhoneStateListener? = null

    /** Per the plan §6 `CALL_GRAB_FALLBACK = 2_000 ms`. If the buds
     *  haven't reached the phone in time, flip
     *  [AudioManager.setSpeakerphoneOn] so the call is audible
     *  regardless. */
    private val callGrabFallbackMs: Long = 2_000

    /** Start listening to phone-state changes. Caller is responsible
     *  for granting `READ_PHONE_STATE` first — without it the API
     *  silently no-ops (Android throws SecurityException on register).
     *  Returns false if the permission is missing so the service can
     *  surface that to the UI. */
    fun start(): Boolean {
        // Idempotent: the service may call start() again after the user
        // grants READ_PHONE_STATE post-launch (see VortexService
        // ACTION_REFRESH_CALLFLOW). If we're already listening, don't
        // register a second telephony callback.
        if (registeredCallback != null || registeredListener != null) {
            return true
        }
        if (!hasReadPhoneState()) {
            Log.w(tag, "READ_PHONE_STATE missing; call handoff disabled")
            return false
        }
        val tm = context.getSystemService(Context.TELEPHONY_SERVICE) as? TelephonyManager
        if (tm == null) {
            Log.w(tag, "TelephonyManager unavailable (no telephony hardware?)")
            return false
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // Android 12+ — the modern callback API.
            val cb = object : TelephonyCallback(), TelephonyCallback.CallStateListener {
                override fun onCallStateChanged(state: Int) {
                    handleCallState(state)
                }
            }
            try {
                tm.registerTelephonyCallback(context.mainExecutor, cb)
                registeredCallback = cb
                Log.i(tag, "telephony callback registered (API 31+)")
            } catch (e: SecurityException) {
                Log.w(tag, "registerTelephonyCallback rejected: ${e.message}")
                return false
            }
        } else {
            // Pre-Android-12 PhoneStateListener fallback. Deprecated
            // but still functional through API 30.
            @Suppress("DEPRECATION")
            val listener = object : PhoneStateListener() {
                @Deprecated("Pre-S")
                override fun onCallStateChanged(state: Int, phoneNumber: String?) {
                    handleCallState(state, phoneNumber)
                }
            }
            try {
                @Suppress("DEPRECATION")
                tm.listen(listener, PhoneStateListener.LISTEN_CALL_STATE)
                registeredListener = listener
                Log.i(tag, "phone-state listener registered (pre-API-31)")
            } catch (e: SecurityException) {
                Log.w(tag, "listen rejected: ${e.message}")
                return false
            }
        }
        return true
    }

    fun stop() {
        val tm = context.getSystemService(Context.TELEPHONY_SERVICE) as? TelephonyManager
        if (tm != null) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                registeredCallback?.let {
                    try { tm.unregisterTelephonyCallback(it) } catch (_: Exception) { /* fine */ }
                }
            } else {
                registeredListener?.let {
                    @Suppress("DEPRECATION")
                    try { tm.listen(it, PhoneStateListener.LISTEN_NONE) } catch (_: Exception) { /* fine */ }
                }
            }
        }
        registeredCallback = null
        registeredListener = null
        speakerphoneFallbackJob?.cancel()
        scope.coroutineContext[Job]?.cancel()
    }

    /** Tell the orchestrator the buds actually reached the phone so
     *  we can cancel the speakerphone fallback AND disable
     *  speakerphone if the fallback already fired. Called from the
     *  switch orchestrator when its state goes to Idle / AlmostDone
     *  during an active call. */
    fun notifyBudsConnected() {
        speakerphoneFallbackJob?.cancel()
        speakerphoneFallbackJob = null
        // The fallback may have already enabled speakerphone — if the
        // buds arrived late we'd otherwise leave the user hearing the
        // caller through both speaker and earbuds. Disable
        // unconditionally; it's a no-op when speakerphone wasn't on.
        val phase = phaseFlow.value
        if (phase == CallPhase.Ringing || phase == CallPhase.Active) {
            disableSpeakerphone()
        }
    }

    /** Debug-only entrypoint: inject a synthetic call-state transition
     *  so live tests (`adb shell am broadcast …`) can drive the call
     *  handoff without needing a real phone call. The path is the
     *  same as the real one — same callbacks, same speakerphone
     *  fallback — so a green test exercises the production flow. */
    fun simulateCallStateForDebug(state: Int, incomingNumber: String? = null) {
        Log.w(tag, "simulateCallStateForDebug($state) — debug only")
        handleCallState(state, incomingNumber)
    }

    /** Mirror of [TelephonyManager.CALL_STATE_*] → our enum. Routes
     *  CallPhase transitions and fires the matching callback. */
    private fun handleCallState(rawState: Int, incomingNumber: String? = null) {
        val nextPhase = when (rawState) {
            TelephonyManager.CALL_STATE_RINGING -> CallPhase.Ringing
            TelephonyManager.CALL_STATE_OFFHOOK -> CallPhase.Active
            TelephonyManager.CALL_STATE_IDLE -> CallPhase.Idle
            else -> return
        }
        val prev = phaseFlow.value
        if (prev == nextPhase) return
        Log.i(tag, "call $prev → $nextPhase")
        phaseFlow.value = nextPhase

        when {
            prev == CallPhase.Idle && (nextPhase == CallPhase.Ringing || nextPhase == CallPhase.Active) -> {
                // New call entering the system. Trigger the handoff, and
                // arm the speakerphone fallback ONLY if a grab actually
                // started — with the buds already on the phone (or BT off)
                // there is no in-flight route gap to bridge, and forcing
                // speakerphone would yank the call OFF a working route.
                emitCallEnter(nextPhase, incomingNumber)
                scope.launch {
                    val handoffStarted = try {
                        onCallStart()
                    } catch (e: Exception) {
                        Log.w(tag, "onCallStart threw: ${e.message}")
                        false
                    }
                    if (handoffStarted) armSpeakerphoneFallback()
                }
            }
            prev == CallPhase.Ringing && nextPhase == CallPhase.Active -> {
                // Incoming call answered → mirror the in-call pill (caller +
                // a start time the laptop ticks locally). The handoff was
                // already kicked off when we entered Ringing.
                emitCallActive()
            }
            (prev == CallPhase.Ringing || prev == CallPhase.Active) && nextPhase == CallPhase.Idle -> {
                // Call wrapped up. Clear speakerphone (we own it from
                // the fallback if it fired) and trigger the resume.
                phaseFlow.value = CallPhase.EndingHandoff
                emitCallEnded()
                speakerphoneFallbackJob?.cancel()
                speakerphoneFallbackJob = null
                disableSpeakerphone()
                scope.launch {
                    try { onCallEnd() } catch (e: Exception) { Log.w(tag, "onCallEnd threw: ${e.message}") }
                    // Settle back to Idle. The buds reroute is driven
                    // by onCallEnd; whether it succeeds doesn't change
                    // the phone's view that the call has ended.
                    phaseFlow.value = CallPhase.Idle
                }
            }
        }
    }

    /** New call entering: build the [CallEvent] (resolve the caller name off
     *  the main thread), stash it for the active/ended updates, emit it. An
     *  OFFHOOK-from-idle is an outgoing call → straight to active. */
    private fun emitCallEnter(phase: CallPhase, incomingNumber: String?) {
        val outgoing = phase == CallPhase.Active
        val id = "call-${System.currentTimeMillis()}"
        val number = incomingNumber?.trim().orEmpty()
        val dialerPkg = defaultDialerPackage()
        // Stamp `current` SYNCHRONOUSLY (name not yet resolved) so a fast
        // hang-up's emitCallEnded() always has a call to end. Resolving the
        // contact name is async (ContactsContract off the main thread); without
        // the sync stamp, a quick call that ends before the name resolves would
        // leave current=null → emitCallEnded() no-ops → the ENDED event never
        // fires → the laptop's "Calling…" pill is stranded forever (its 30 s
        // keep-alive holds it past the staleness sweeper). Seen with outgoing
        // calls dialled from the laptop that the callee drops quickly.
        val base = CallEvent(
            id = id,
            phase = if (outgoing) CallEvent.PHASE_ACTIVE else CallEvent.PHASE_RINGING,
            name = "",
            number = number,
            startedAt = if (outgoing) System.currentTimeMillis() else 0L,
            outgoing = outgoing,
            appId = dialerPkg,
        )
        current = base
        // Resolve the name off the main thread, then emit ONCE (single emit
        // keeps the laptop's (id,phase) dedup happy — no name-less flash). Skip
        // the emit if the call already ended in the meantime: emitCallEnded()
        // nulled `current` and we must not resurrect the pill with a late event.
        scope.launch {
            val name = resolveContactName(number)
            val cur = current ?: return@launch
            if (cur.id != id) return@launch
            val ev = if (name.isEmpty()) cur else cur.copy(name = name)
            current = ev
            safeEmit(ev)
        }
    }

    /** Incoming call answered → emit phase=active with the connect time. */
    private fun emitCallActive() {
        val base = current ?: return
        val ev = base.copy(phase = CallEvent.PHASE_ACTIVE, startedAt = System.currentTimeMillis())
        current = ev
        safeEmit(ev)
    }

    /** Call ended → emit phase=ended (laptop clears banner/pill), clear state. */
    private fun emitCallEnded() {
        val base = current ?: return
        current = null
        safeEmit(base.copy(phase = CallEvent.PHASE_ENDED))
    }

    private fun safeEmit(ev: CallEvent) {
        try {
            onCallEvent(ev)
        } catch (e: Exception) {
            Log.w(tag, "onCallEvent threw: ${e.message}")
        }
    }

    /** Resolve a number to a contact display name via [ContactsContract]
     *  PhoneLookup. Returns "" when READ_CONTACTS is missing, the number is
     *  empty/withheld, or no contact matches (the laptop then shows the
     *  number). Never throws. */
    private fun resolveContactName(number: String): String {
        if (number.isEmpty()) return ""
        if (ContextCompat.checkSelfPermission(context, android.Manifest.permission.READ_CONTACTS)
            != PackageManager.PERMISSION_GRANTED
        ) {
            return ""
        }
        return try {
            val uri = Uri.withAppendedPath(
                ContactsContract.PhoneLookup.CONTENT_FILTER_URI,
                Uri.encode(number),
            )
            context.contentResolver.query(
                uri,
                arrayOf(ContactsContract.PhoneLookup.DISPLAY_NAME),
                null, null, null,
            )?.use { c ->
                if (c.moveToFirst()) c.getString(0)?.trim().orEmpty() else ""
            }.orEmpty()
        } catch (e: Exception) {
            Log.w(tag, "resolveContactName failed: ${e.message}")
            ""
        }
    }

    /** Set a 2-second deadline. If the buds haven't connected by
     *  then we flip speakerphone on so the conversation is audible.
     *  The job is cancellable via [notifyBudsConnected] (and
     *  unconditionally cancelled on call-end). */
    private fun armSpeakerphoneFallback() {
        speakerphoneFallbackJob?.cancel()
        speakerphoneFallbackJob = scope.launch {
            delay(callGrabFallbackMs)
            // Re-check that the call is still active. A short call
            // that ended within the 2 s window must NOT leave the
            // speaker on.
            val phase = phaseFlow.value
            if (phase == CallPhase.Ringing || phase == CallPhase.Active) {
                Log.w(tag, "buds not connected within ${callGrabFallbackMs}ms; enabling speakerphone")
                enableSpeakerphone()
                // Close the cancel race: the IDLE handler may have run
                // between our phase read and the enable — its cancel()
                // missed us and its disableSpeakerphone() ran BEFORE our
                // enable, leaving the speaker stuck on with the call
                // over. The IDLE handler writes the phase before it
                // disables, so re-reading here catches every interleaving.
                val now = phaseFlow.value
                if (now != CallPhase.Ringing && now != CallPhase.Active) {
                    Log.w(tag, "call ended during speaker fallback; undoing")
                    disableSpeakerphone()
                }
            }
        }
    }

    private fun enableSpeakerphone() {
        try {
            val am = context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
            // MODE_IN_COMMUNICATION + speakerphoneOn = audible call audio
            // through the loudspeaker even if BT was the previous route.
            am.mode = AudioManager.MODE_IN_COMMUNICATION
            am.isSpeakerphoneOn = true
        } catch (e: Exception) {
            Log.w(tag, "enableSpeakerphone failed: ${e.message}")
        }
    }

    private fun disableSpeakerphone() {
        try {
            val am = context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
            am.isSpeakerphoneOn = false
            am.mode = AudioManager.MODE_NORMAL
        } catch (e: Exception) {
            Log.w(tag, "disableSpeakerphone failed: ${e.message}")
        }
    }

    /** The phone's default dialer package (its Phone-app launcher icon is what
     *  the laptop shows on the call banner/pill). "" if unavailable. */
    private fun defaultDialerPackage(): String = try {
        val tm = context.getSystemService(Context.TELECOM_SERVICE) as? TelecomManager
        tm?.defaultDialerPackage.orEmpty()
    } catch (_: Exception) {
        ""
    }

    private fun hasReadPhoneState(): Boolean =
        ContextCompat.checkSelfPermission(
            context,
            android.Manifest.permission.READ_PHONE_STATE,
        ) == PackageManager.PERMISSION_GRANTED
}
