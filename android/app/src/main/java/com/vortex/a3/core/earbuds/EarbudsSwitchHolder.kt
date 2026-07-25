package com.vortex.a3.core.earbuds

import android.content.Context
import android.util.Log
import com.vortex.a3.core.storage.PeerStore
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.util.concurrent.atomic.AtomicInteger

/**
 * Process-wide holder for the earbuds [SwitchOrchestrator].
 *
 * The orchestrator must outlive any single Activity (state machine
 * progress can't be lost on rotation) and must be reachable from both
 * the UI (MainActivity, for tap → request) and the transport
 * (LanServer / GattServer, for frame ingress). Making it an `object`
 * keeps the lifecycle to one instance for the whole process — there
 * is at most one set of buds we're juggling at a time on V1.
 *
 * [init] is idempotent: VortexService and MainActivity may both call
 * it on different code paths; the first one wins. The latter
 * `peerStore` instance is ignored — both come from the same
 * EncryptedSharedPreferences file so the values are identical.
 *
 * The [sender] is a stub for Phase 1.7 — it logs the frame and
 * returns failure, which the orchestrator surfaces as a Failed state
 * (auto-resets to Idle after 3 s). Phase 1.8 swaps in the real
 * LAN/BLE transport so the frame actually reaches the peer.
 */
object EarbudsSwitchHolder {

    private const val TAG = "EarbudsSwitchHolder"

    private val notReadyState = MutableStateFlow<SwitchState>(SwitchState.Idle)

    @Volatile private var orchestratorRef: SwitchOrchestrator? = null
    @Volatile private var controllerRef: AudioDeviceController? = null

    /** Session writers keyed by peer-public hex. Populated by the
     *  LAN server (and, later, the BLE responder path) for the
     *  lifetime of a single accepted TCP+IK session so orchestrator
     *  responses (Approve / Released / Done) ride back on the same
     *  socket. V1 is single-peer, so there is at most one entry. */
    private val sessionWriters =
        java.util.concurrent.ConcurrentHashMap<String, suspend (AudioOpFrame) -> Result<Unit>>()

    /** BLE-NOTIFY writers keyed by peer-public hex. Populated once per
     *  trusted reconnect (P2.13) so the orchestrator can push AUDIO_OP
     *  frames in ~200 ms via the persistent GATT link instead of waiting
     *  for a LAN session to spin up. Distinct from [sessionWriters] so
     *  one channel coming and going doesn't trample the other. */
    private val bleWriters =
        java.util.concurrent.ConcurrentHashMap<String, suspend (AudioOpFrame) -> Result<Unit>>()

    /** Background scope for the LOSER write of a BLE+LAN race. We must
     *  not cancel it when the winner returns — a half-written LAN TCP
     *  frame would corrupt the stream — so it finishes detached here. */
    private val raceScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    /** Send `frame` over BLE and LAN concurrently, returning on the
     *  FIRST success (or the last failure if both fail). The loser keeps
     *  running in [raceScope] until it completes.
     *
     *  Why race instead of BLE-first: during a call hand-off the buds
     *  stream A2DP to whoever holds them, and A2DP starves the BLE radio
     *  on that side — so the inbound `Request` to the laptop crawls over
     *  BLE (~0.7 s) while Wi-Fi/LAN is wide open. BLE-first never tried
     *  LAN because the BLE write *eventually succeeds*, just slowly. The
     *  receiver dedups by `frame.nonce` (the slower copy is dropped as a
     *  replay), so double-delivery is safe. Mirrors the Linux sender
     *  race (commit 0cef0a8). */
    private suspend fun raceSend(
        ble: suspend (AudioOpFrame) -> Result<Unit>,
        lan: suspend (AudioOpFrame) -> Result<Unit>,
        frame: AudioOpFrame,
    ): Result<Unit> {
        val winner = CompletableDeferred<Result<Unit>>()
        val remaining = AtomicInteger(2)
        for (w in listOf(ble, lan)) {
            raceScope.launch {
                val r = runCatching { w(frame) }.getOrElse { Result.failure(it) }
                if (r.isSuccess) {
                    winner.complete(r) // idempotent: only the first success wins
                } else if (remaining.decrementAndGet() == 0) {
                    winner.complete(r) // both failed → surface the last error
                }
            }
        }
        return winner.await()
    }

    /** Idempotent. Safe to call from VortexService.startStack() and
     *  MainActivity.startStack() — only the first one initializes;
     *  subsequent calls are no-ops. */
    /** Phase 2 — call-aware owner vote. VortexStack wires this to reject an
     *  incoming bud Request while a phone call is ringing/active: the call
     *  outranks media, and approving would yank the call audio mid-sentence.
     *  Volatile lambda (not a constructor arg) because the holder may init
     *  before the call flow starts. Defaults to Allow until wired. */
    @Volatile
    private var acceptance: () -> SwitchOrchestrator.Acceptance =
        { SwitchOrchestrator.Acceptance.Allow }

    fun setAcceptanceProvider(provider: () -> SwitchOrchestrator.Acceptance) {
        acceptance = provider
    }

    @Synchronized
    fun init(context: Context, peerStore: PeerStore) {
        if (orchestratorRef != null) return
        val controller = AudioDeviceController(context.applicationContext)
        controller.prewarm()  // S1 — pre-warmed BT profile proxy
        val persistence = SwitchPersistence(context.applicationContext)
        val orchestrator = SwitchOrchestrator(
            controller = controller,
            peerStore = peerStore,
            sender = { peerPub, frame ->
                // P2.13: prefer the persistent BLE writer (registered
                // once at IK-reconnect time, no per-frame setup cost,
                // ~200 ms RTT) and fall back to whichever LAN session
                // writer happens to be live. LAN writer existence is
                // tied to an in-flight TCP+IK socket, which usually
                // isn't there when a call comes in — that 5-second
                // gap was the whole reason for the BLE path.
                val key = peerPub.joinToString("") { "%02x".format(it) }
                val bleWriter = bleWriters[key]
                val lanWriter = sessionWriters[key]
                when {
                    // Both transports live → race them. Wins whichever
                    // the radio conditions favour (LAN usually, during a
                    // call, since the buds' A2DP starves BLE on the
                    // holder side). Receiver dedups by nonce.
                    bleWriter != null && lanWriter != null ->
                        raceSend(bleWriter, lanWriter, frame)
                    bleWriter != null -> bleWriter(frame)
                    lanWriter != null -> lanWriter(frame)
                    else -> {
                        Log.w(TAG, "no writer (BLE or LAN) for peer=${key.take(8)}… op=${frame.op}")
                        Result.failure(IllegalStateException("no active session; need either subscribed AUDIO_SIGNAL or open LAN socket"))
                    }
                }
            },
            acceptanceProvider = { acceptance() },
            persistence = persistence,
        )
        controllerRef = controller
        orchestratorRef = orchestrator
        // R1 — recover any in-flight switch state persisted from a
        // previous process. Runs once, here.
        orchestrator.recoverOnStart()
        Log.i(TAG, "initialized")
    }

    /** State flow for the UI to observe. Returns a static Idle flow
     *  if [init] hasn't been called yet — the EarbudsCard renders the
     *  same way as if no flow is in progress. */
    val state: StateFlow<SwitchState>
        get() = orchestratorRef?.state ?: notReadyState.asStateFlow()

    /** Returns false if the holder isn't initialized or a flow is
     *  already in progress. The UI shows the existing state and
     *  silently drops the tap (no error toast) — matches the "two
     *  rapid taps collapse to one switch" acceptance criterion. */
    fun request(peerPub: ByteArray, mac: String): Boolean {
        val o = orchestratorRef ?: run {
            Log.w(TAG, "request before init")
            return false
        }
        return o.request(peerPub, mac)
    }

    /** Fast call-end claim — phone announces "buds are yours" over BLE
     *  and releases locally so the laptop's connect lands on the first
     *  attempt. See [SwitchOrchestrator.claim] for the rationale. */
    fun claim(peerPub: ByteArray, mac: String) {
        val o = orchestratorRef ?: run {
            Log.w(TAG, "claim before init")
            return
        }
        o.claim(peerPub, mac)
    }

    /** Entry for incoming AUDIO_OP frames. Called by LanServer /
     *  GattServer when an AEAD-decrypted [AudioOpFrame] arrives.
     *  Drops silently if the orchestrator isn't up yet — the peer
     *  will time out and retry. */
    suspend fun onIncoming(peerPub: ByteArray, frame: AudioOpFrame) {
        val o = orchestratorRef ?: run {
            Log.w(TAG, "onIncoming before init")
            return
        }
        o.onIncoming(peerPub, frame)
    }

    /** Test-only hook so unit tests of MainActivity composables can
     *  inject a fake orchestrator. Not for production use. */
    @androidx.annotation.VisibleForTesting
    @Synchronized
    fun replaceForTest(orchestrator: SwitchOrchestrator?) {
        orchestratorRef = orchestrator
    }

    /** Register a session writer for the accepted-LAN-connection's
     *  lifetime. Called by [com.vortex.a3.core.lan.LanServer] when
     *  the first AUDIO_OP frame arrives — heartbeat-only connections
     *  never claim the slot, so the writer remains pointed at the
     *  socket that actually carries the switch flow. */
    fun setSessionWriter(peerPub: ByteArray, writer: suspend (AudioOpFrame) -> Result<Unit>) {
        val key = peerPub.joinToString("") { "%02x".format(it) }
        sessionWriters[key] = writer
    }

    /** Drop the writer when the connection closes — but only if the
     *  slot still holds the same writer instance the caller registered.
     *  Prevents a heartbeat finishing after the audio session started
     *  from racing into a stale `clear`. Idempotent. */
    fun clearSessionWriter(peerPub: ByteArray, writer: suspend (AudioOpFrame) -> Result<Unit>) {
        val key = peerPub.joinToString("") { "%02x".format(it) }
        sessionWriters.remove(key, writer)
    }

    /** Register the BLE-NOTIFY writer for [peerPub] (P2.13). Survives
     *  individual LAN sessions; cleared only on un-trust / process
     *  death / reconnect (which re-registers a fresh cipher state). */
    fun setBleWriter(peerPub: ByteArray, writer: suspend (AudioOpFrame) -> Result<Unit>) {
        val key = peerPub.joinToString("") { "%02x".format(it) }
        bleWriters[key] = writer
    }

    /** Drop the BLE-NOTIFY writer (CAS, same shape as
     *  [clearSessionWriter]). Called when the IK transport state is
     *  invalidated — e.g. after a fresh reconnect replaces it. */
    fun clearBleWriter(peerPub: ByteArray, writer: suspend (AudioOpFrame) -> Result<Unit>) {
        val key = peerPub.joinToString("") { "%02x".format(it) }
        bleWriters.remove(key, writer)
    }
}

