package com.vortex.a3.core.pairing

import android.bluetooth.BluetoothDevice
import android.util.Log
import com.southernstorm.noise.protocol.CipherState
import com.southernstorm.noise.protocol.CipherStatePair
import com.southernstorm.noise.protocol.HandshakeState
import com.southernstorm.noise.protocol.Noise
import com.vortex.a3.core.ble.Frame
import com.vortex.a3.core.ble.FrameSub
import com.vortex.a3.core.ble.FrameType
import com.vortex.a3.core.crypto.Derive
import com.vortex.a3.core.crypto.NoiseRunner
import com.vortex.a3.core.crypto.Sas
import com.vortex.a3.core.identity.IdentityRecord
import java.util.Collections
import java.util.LinkedHashMap

/**
 * Per-device Noise XX **responder** state machine for first pairing
 * (spec §6.1, §6.4).
 *
 * The orchestrator owns the long-lived static private key and dispatches
 * Pairing Control frames received over GATT. It does NOT itself touch
 * BLE — callers (GattServer) feed it Frames and consume the produced
 * outbound Frames.
 */
class PairingOrchestrator(private val identity: IdentityRecord) {

    /** Result emitted to listeners after each phase transition. */
    data class HandshakeOutcome(
        val device: BluetoothDevice,
        val transcriptHash: ByteArray,
        val peerStaticPub: ByteArray,
        val sasString: String,
        val state: PhaseState,
        /** Peer's friendly device name, carried in the APPROVE payload. */
        val peerName: String? = null,
    )

    /** Approval / pairing phase. */
    enum class PhaseState {
        XxComplete,        // Noise XX finished, awaiting peer's approval frame.
        BothApproved,      // Both sides approved; PRS derived.
        PeerRejected,      // Peer sent a reject frame.
    }

    /** Per-side approval state in the dual-approval window. */
    private enum class Decision { Pending, Approve, Reject }

    /** Per-device state machine. */
    private sealed class XxState {
        object Idle : XxState()
        class WaitingForMsg3(val handshake: HandshakeState) : XxState()
        class AwaitingApprovals(
            val transcriptHash: ByteArray,
            val peerStaticPub: ByteArray,
            val sasString: String,
            /** Noise transport cipher used to AEAD-wrap our outbound frames. */
            val sender: CipherState,
            /** Noise transport cipher used to AEAD-unwrap inbound frames. */
            val receiver: CipherState,
            /** Wall-clock deadline after which the orchestrator auto-rejects. */
            val deadlineMs: Long = System.currentTimeMillis() + APPROVAL_TIMEOUT_MS,
            var localDecision: Decision = Decision.Pending,
            var peerDecision: Decision = Decision.Pending,
            var peerName: String? = null,
        ) : XxState()
        class Completed(
            val transcriptHash: ByteArray,
            val peerStaticPub: ByteArray,
            val prs: ByteArray,
            val peerName: String? = null,
        ) : XxState()
        class Rejected(val transcriptHash: ByteArray) : XxState()
    }

    /**
     * Dev / test-only auto-approve. When true, the orchestrator immediately
     * builds an `approve` frame after XX completes. Production builds leave
     * this `false` and require an explicit [userApprove] / [userReject]
     * call from the UI per spec §6 (dual approval).
     */
    var autoApprove: Boolean = false

    /**
     * Per-device handshake state. Bounded to MAX_CONCURRENT_PEERS so a
     * BD_ADDR-rotating attacker cannot drive the map to OOM by spraying
     * msg1 frames with randomized addresses.
     *
     * Eviction policy is "oldest wins-by-insertion-order" — a fresh peer
     * starting handshake displaces the oldest stalled one. Live pairings
     * are bounded to one at a time in normal UX so eviction is benign.
     */
    private val states: MutableMap<String, XxState> = Collections.synchronizedMap(
        object : LinkedHashMap<String, XxState>(16, 0.75f, /*accessOrder=*/false) {
            override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, XxState>): Boolean {
                return size > MAX_CONCURRENT_PEERS
            }
        }
    )
    private val listeners: MutableList<(HandshakeOutcome) -> Unit> = mutableListOf()

    fun addListener(listener: (HandshakeOutcome) -> Unit) {
        synchronized(listeners) { listeners.add(listener) }
    }

    /** Reset the state for a peer (e.g., on disconnect). */
    fun forgetDevice(device: BluetoothDevice) {
        states.remove(device.address)
    }

    /**
     * Reset a peer's IN-PROGRESS handshake when its GATT link drops: a dead
     * link can never finish msg3 / deliver an approval, and the stale entry
     * would make the in-flight-msg1 guard reject a retry from the same
     * address for up to the 30s sweep. A [XxState.Completed] entry survives —
     * MainActivity reads its PRS via [peerPrs] right after BothApproved, and
     * the laptop may legitimately disconnect in that window.
     */
    fun forgetDeviceOnDisconnect(device: BluetoothDevice) {
        val s = states[device.address] ?: return
        if (s is XxState.Completed) return
        states.remove(device.address)
        Log.i(TAG, "pairing state cleared on disconnect for ${device.address}")
    }

    /** Returns the derived PRS for [deviceAddress] if pairing has completed. */
    fun peerPrs(deviceAddress: String): ByteArray? {
        val s = states[deviceAddress]
        return if (s is XxState.Completed) s.prs.copyOf() else null
    }

    /**
     * Returns true iff the local SAS dialog should currently be open for
     * [deviceAddress] — i.e., XX is complete and the orchestrator is
     * awaiting a user decision on this device.
     */
    fun isAwaitingUserDecision(deviceAddress: String): Boolean =
        states[deviceAddress] is XxState.AwaitingApprovals

    /**
     * Convenience combined API: returns the outbound APPROVE frame and
     * commits the local decision. Intended for tests; production UI
     * code (MainActivity) MUST use [buildLocalApprovalFrame] +
     * [commitLocalDecision] separately so the frame can be flushed
     * over the GATT server BEFORE listeners tear the server down.
     *
     * Pass [localName] to embed our friendly device name in the APPROVE
     * payload — production callers always supply this so the peer sees
     * a real label. Tests can omit it.
     */
    fun userApprove(device: BluetoothDevice, localName: String? = null): Frame? {
        val frame = buildLocalApprovalFrame(device, approve = true, localName = localName)
            ?: run {
                Log.w(TAG, "userApprove for ${device.address} in unexpected state")
                return null
            }
        commitLocalDecision(device, approve = true)
        return frame
    }

    /**
     * Convenience combined API for the wrong-SAS path; see [userApprove].
     */
    fun userReject(device: BluetoothDevice): Frame? {
        val frame = buildLocalApprovalFrame(device, approve = false)
            ?: run {
                Log.w(TAG, "userReject for ${device.address} in unexpected state")
                return null
            }
        commitLocalDecision(device, approve = false)
        return frame
    }

    /**
     * Build the outbound APPROVE / REJECT frame for [device] **without**
     * mutating state. Returns null if no pairing is awaiting a local
     * decision for this device.
     *
     * Callers that wire a listener which tears down the GATT server on
     * `BothApproved` MUST send this frame BEFORE calling
     * [commitLocalDecision]; otherwise the listener fires inside the
     * commit and stops the GATT server before the notification flies
     * out, leaving the peer hung on its half of the handshake.
     */
    fun buildLocalApprovalFrame(
        device: BluetoothDevice,
        approve: Boolean,
        localName: String? = null,
    ): Frame? {
        val s = states[device.address] as? XxState.AwaitingApprovals ?: return null
        val sub = if (approve) APPROVAL_APPROVE else APPROVAL_REJECT
        // On approve, carry our friendly device name as utf-8 payload so
        // the Linux peer can label this device as e.g. "Redmi Note 11"
        // instead of "Android phone". Empty payload on reject. The name
        // is sanitized + length-capped before serialization so a future
        // bug pushing garbage / hostile bytes into Build.MODEL cannot
        // leak control chars or RTL overrides to the peer.
        val plain = if (approve && !localName.isNullOrBlank()) {
            sanitizePeerName(localName).toByteArray(Charsets.UTF_8)
        } else {
            ByteArray(0)
        }
        // AEAD-wrap with the Noise transport sender cipher derived in
        // [handleMsg3]. The peer verifies the tag with its receiver
        // cipher; tampered or replayed frames fail decrypt.
        val payload = aeadSeal(s.sender, plain)
        return Frame(FrameType.PAIRING_APPROVAL, sub, payload)
    }

    /**
     * Record the local user's APPROVE / REJECT decision. May synchronously
     * fire `BothApproved` or `PeerRejected` listeners (which, in
     * production, tear down the Activity-local GATT server). Send the
     * frame returned by [buildLocalApprovalFrame] BEFORE calling this.
     *
     * userApprove (UI thread) and handlePeerApproval (GATT binder
     * thread) both mutate the same `s.localDecision` / `s.peerDecision`
     * fields. Without synchronisation on `s` they can each observe
     * `both=Approve` and double-emit `BothApproved`. The block below
     * makes the mutation + finalize atomic per device.
     */
    fun commitLocalDecision(device: BluetoothDevice, approve: Boolean) {
        val s = states[device.address] as? XxState.AwaitingApprovals ?: return
        synchronized(s) {
            if (states[device.address] !== s) return
            Log.i(
                TAG,
                "user ${if (approve) "approved" else "rejected"} pairing for ${device.address}",
            )
            s.localDecision = if (approve) Decision.Approve else Decision.Reject
            finalizeIfReady(device, s)
        }
    }

    /**
     * Process an incoming Pairing Control frame for [device].
     *
     * Returns a Frame to send back as a Notification, or null if no
     * outbound response is needed.
     */
    fun onPairingControlFrame(device: BluetoothDevice, frame: Frame): Frame? {
        // Sweep any expired AwaitingApprovals entries on each inbound
        // frame. Cheap (constant time per peer) and avoids needing a
        // background timer just to keep the state machine honest.
        sweepExpiredApprovals()
        return when (frame.type) {
            FrameType.PAIRING_HANDSHAKE -> when (frame.sub) {
                HANDSHAKE_MSG1 -> handleMsg1(device, frame.payload)
                HANDSHAKE_MSG3 -> handleMsg3(device, frame.payload)
                else -> {
                    Log.w(TAG, "unexpected handshake sub=0x${"%02x".format(frame.sub)} for ${device.address}")
                    null
                }
            }
            FrameType.PAIRING_APPROVAL -> handlePeerApproval(device, frame.sub, frame.payload)
            else -> {
                Log.w(TAG, "ignoring frame type=0x${"%02x".format(frame.type)} on Pairing Control")
                null
            }
        }
    }

    private fun handleMsg1(device: BluetoothDevice, payload: ByteArray): Frame? {
        Log.i(TAG, "handshake msg1 from ${device.address}: ${payload.size} bytes")
        // **In-flight guard (ChatGPT review #4).** If we already have a
        // handshake in progress for this address (msg2 sent, waiting
        // on msg3 or on user approval), a second msg1 must NOT
        // overwrite the live state. Previously the new msg1 silently
        // replaced the in-flight transcript / SAS — an attacker with
        // BLE range could swap the SAS the user is about to verify,
        // and a quick re-pair tap turns into a misdirected trust.
        // Drop the duplicate msg1; the peer's retry will land after
        // our sweep expires the in-flight entry.
        when (val cur = states[device.address]) {
            is XxState.WaitingForMsg3 -> {
                Log.w(TAG, "msg1 dropped: already WaitingForMsg3 for ${device.address}")
                return null
            }
            is XxState.AwaitingApprovals -> {
                Log.w(TAG, "msg1 dropped: already AwaitingApprovals for ${device.address}")
                return null
            }
            else -> { /* Idle or absent — proceed */ Unit }
        }
        val handshake = try {
            buildResponder()
        } catch (e: Exception) {
            Log.e(TAG, "responder build failed", e)
            states[device.address] = XxState.Idle
            return null
        }

        // read msg1
        val readBuf = ByteArray(Noise.MAX_PACKET_LEN)
        try {
            handshake.readMessage(payload, 0, payload.size, readBuf, 0)
        } catch (e: Exception) {
            Log.e(TAG, "read msg1 failed", e)
            handshake.destroy() // zero key material on the abandoned handshake
            return null
        }

        // write msg2
        val writeBuf = ByteArray(Noise.MAX_PACKET_LEN)
        val n = try {
            handshake.writeMessage(writeBuf, 0, null, 0, 0)
        } catch (e: Exception) {
            Log.e(TAG, "write msg2 failed", e)
            handshake.destroy() // zero key material on the abandoned handshake
            return null
        }
        val msg2Bytes = writeBuf.copyOf(n)
        Log.i(TAG, "handshake msg2 to ${device.address}: $n bytes")

        states[device.address] = XxState.WaitingForMsg3(handshake)
        return Frame(FrameType.PAIRING_HANDSHAKE, HANDSHAKE_MSG2, msg2Bytes)
    }

    private fun handleMsg3(device: BluetoothDevice, payload: ByteArray): Frame? {
        Log.i(TAG, "handshake msg3 from ${device.address}: ${payload.size} bytes")
        val state = states[device.address]
        if (state !is XxState.WaitingForMsg3) {
            Log.w(TAG, "msg3 in unexpected state: $state")
            return null
        }

        val readBuf = ByteArray(Noise.MAX_PACKET_LEN)
        try {
            state.handshake.readMessage(payload, 0, payload.size, readBuf, 0)
        } catch (e: Exception) {
            Log.e(TAG, "read msg3 failed", e)
            state.handshake.destroy() // zero key material on the abandoned handshake
            states[device.address] = XxState.Idle
            return null
        }

        val transcript = state.handshake.handshakeHash.copyOf()
        val peerStaticPub = ByteArray(32).also { state.handshake.remotePublicKey.getPublicKey(it, 0) }
        val (_, sasString) = Sas.derive(transcript)

        // Promote the Noise handshake to transport mode. Every
        // post-handshake frame (APPROVE / REJECT and any V2
        // application data) MUST flow through AEAD so a network
        // attacker can't tamper with the dual-approval gate.
        val pair: CipherStatePair = try {
            state.handshake.split()
        } catch (e: Exception) {
            Log.e(TAG, "split() failed", e)
            state.handshake.destroy() // zero key material on the error path
            states[device.address] = XxState.Idle
            return null
        }
        val sender: CipherState = pair.sender
        val receiver: CipherState = pair.receiver
        // split() copied what the transport ciphers need; zero the handshake's
        // static-priv copy + ephemeral now (GC never wipes).
        state.handshake.destroy()

        Log.i(TAG, "✅ XX complete for ${device.address}")
        // Redaction (spec §3.5 T-LOC-3): SAS, PRS, static_priv, presence
        // tokens MUST NOT appear in production logs. Transcript hash and
        // peer_static_pub are public protocol values, but we log only short
        // prefixes to avoid stable-identity correlation in field logs.
        Log.i(TAG, "   transcript_hash = ${transcript.toHexPrefix()}")
        Log.i(TAG, "   peer_static_pub = ${peerStaticPub.toHexPrefix()}")
        Log.i(TAG, "   SAS             = [redacted; shown in UI only]")

        val approvals = XxState.AwaitingApprovals(
            transcriptHash = transcript,
            peerStaticPub = peerStaticPub,
            sasString = sasString,
            sender = sender,
            receiver = receiver,
        )
        // Dev / test-only auto-approve: pre-record local Approve so the
        // dual-approval finalizer doesn't sit waiting for a UI tap that
        // headless tests will never deliver.
        if (autoApprove) {
            approvals.localDecision = Decision.Approve
        }
        states[device.address] = approvals
        emitOutcome(HandshakeOutcome(device, transcript, peerStaticPub, sasString, PhaseState.XxComplete))

        return if (autoApprove) {
            Log.i(TAG, "auto-approving (dev mode)")
            // Encrypt empty payload — tag-only frame establishes the
            // initiator's session-key check just like the production
            // approval path.
            val ct = aeadSeal(sender, ByteArray(0))
            Frame(FrameType.PAIRING_APPROVAL, APPROVAL_APPROVE, ct)
        } else null
    }

    /**
     * Auto-reject any AwaitingApprovals entry whose deadline has
     * passed. Called opportunistically from [onPairingControlFrame]
     * so an inactive peer cannot pin our handshake state forever.
     */
    private fun sweepExpiredApprovals() {
        val now = System.currentTimeMillis()
        val expired = synchronized(states) {
            states.entries
                .filter { (_, s) -> s is XxState.AwaitingApprovals && now > s.deadlineMs }
                .map { it.key }
        }
        for (addr in expired) {
            val s = states[addr] as? XxState.AwaitingApprovals ?: continue
            Log.w(TAG, "approval timeout for $addr — auto-reject")
            synchronized(s) {
                if (states[addr] !== s) return@synchronized
                s.localDecision = Decision.Reject
                // No frame to send: we don't know if the BLE link is
                // still up at this point, and the peer will see the
                // GATT disconnect / its own approval timeout.
                states[addr] = XxState.Rejected(s.transcriptHash)
            }
        }
    }

    /** AEAD-encrypt [plaintext] with [cipher], no associated data. */
    private fun aeadSeal(cipher: CipherState, plaintext: ByteArray): ByteArray {
        val out = ByteArray(plaintext.size + cipher.macLength)
        val n = cipher.encryptWithAd(null, plaintext, 0, out, 0, plaintext.size)
        return out.copyOf(n)
    }

    /** AEAD-decrypt [ciphertext] with [cipher], no associated data. */
    private fun aeadOpen(cipher: CipherState, ciphertext: ByteArray): ByteArray {
        if (ciphertext.size < cipher.macLength) {
            throw IllegalArgumentException("ciphertext shorter than MAC")
        }
        val out = ByteArray(ciphertext.size)
        val n = cipher.decryptWithAd(null, ciphertext, 0, out, 0, ciphertext.size)
        return out.copyOf(n)
    }

    private fun handlePeerApproval(device: BluetoothDevice, sub: Byte, payload: ByteArray): Frame? {
        val s = states[device.address]
        if (s !is XxState.AwaitingApprovals) {
            Log.w(TAG, "peer approval in unexpected state: $s")
            return null
        }
        synchronized(s) {
            if (states[device.address] !== s) return null
            // Decode sub but DO NOT commit to state yet — a forged frame
            // with sub=APPROVE that fails AEAD must not advance peerDecision.
            // Otherwise a later local Approve would flip us into BothApproved
            // even though no authenticated peer ever approved.
            val pending: Decision = when (sub) {
                APPROVAL_APPROVE -> Decision.Approve
                APPROVAL_REJECT -> Decision.Reject
                else -> {
                    Log.w(TAG, "unexpected approval sub=0x${"%02x".format(sub)}")
                    return null
                }
            }
            // AEAD-unwrap first; only after authenticated decrypt do we
            // commit `peerDecision`. Tampered / replayed frames are dropped
            // with state untouched.
            val plain: ByteArray? = runCatching { aeadOpen(s.receiver, payload) }.getOrNull()
            if (plain == null) {
                Log.w(TAG, "peer approval AEAD-decrypt failed for ${device.address}")
                return null
            }
            s.peerDecision = pending
            when (pending) {
                Decision.Approve -> Log.i(TAG, "peer approved pairing for ${device.address}")
                Decision.Reject -> Log.w(TAG, "peer rejected pairing for ${device.address}")
                else -> {}
            }
            // The APPROVE frame may carry the peer's friendly device
            // name as a utf-8 payload (Linux side sends its hostname,
            // we send Build.MODEL). Empty / invalid payload → null name.
            // Sanitize before storing — peer is post-XX-authenticated
            // but a hostile or buggy implementation could still push
            // control chars / RTL overrides / oversized strings.
            if (pending == Decision.Approve && plain.isNotEmpty()) {
                s.peerName = runCatching {
                    sanitizePeerName(String(plain, Charsets.UTF_8))
                }.getOrNull()?.takeIf { it.isNotEmpty() }
            }
            finalizeIfReady(device, s)
        }
        return null
    }

    /**
     * Driven by both [userApprove] / [userReject] and [handlePeerApproval].
     * Transitions out of [XxState.AwaitingApprovals] iff a final decision
     * is reachable:
     *
     *   - either side rejects → [XxState.Rejected] + `PeerRejected` outcome;
     *   - both sides approve → [XxState.Completed] + `BothApproved` outcome.
     *
     * If only one side has decided and that decision is Approve, we stay
     * in `AwaitingApprovals` waiting for the other side. This is the
     * dual-approval gate cited in spec §3.2 T-PAIR-3.
     */
    private fun finalizeIfReady(device: BluetoothDevice, s: XxState.AwaitingApprovals) {
        val anyReject = s.localDecision == Decision.Reject || s.peerDecision == Decision.Reject
        val bothApprove = s.localDecision == Decision.Approve && s.peerDecision == Decision.Approve
        when {
            anyReject -> {
                states[device.address] = XxState.Rejected(s.transcriptHash)
                emitOutcome(
                    HandshakeOutcome(
                        device, s.transcriptHash, s.peerStaticPub,
                        s.sasString, PhaseState.PeerRejected,
                    )
                )
            }
            bothApprove -> {
                val prs = Derive.prs(s.transcriptHash)
                states[device.address] = XxState.Completed(
                    transcriptHash = s.transcriptHash,
                    peerStaticPub = s.peerStaticPub,
                    prs = prs,
                    peerName = s.peerName,
                )
                Log.i(TAG, "✅ both approved; trust derived for ${device.address}")
                // PRS MUST NOT appear in logs (spec §3.5 T-LOC-3).
                emitOutcome(
                    HandshakeOutcome(
                        device, s.transcriptHash, s.peerStaticPub,
                        s.sasString, PhaseState.BothApproved,
                        peerName = s.peerName,
                    )
                )
            }
            // else: still waiting — no transition.
        }
    }

    private fun emitOutcome(outcome: HandshakeOutcome) {
        synchronized(listeners) {
            for (listener in listeners) listener(outcome)
        }
    }

    private fun buildResponder(): HandshakeState {
        val state = HandshakeState(NoiseRunner.NOISE_XX, HandshakeState.RESPONDER)
        state.setPrologue(NoiseRunner.PROLOGUE_XX, 0, NoiseRunner.PROLOGUE_XX.size)
        state.localKeyPair.setPrivateKey(identity.staticPriv, 0)
        state.start()
        return state
    }

    companion object {
        private const val TAG = "VortexPairing"
        const val HANDSHAKE_MSG1: Byte = 0x01
        const val HANDSHAKE_MSG2: Byte = 0x02
        const val HANDSHAKE_MSG3: Byte = 0x03
        const val APPROVAL_APPROVE: Byte = 0x01
        const val APPROVAL_REJECT: Byte = 0x02

        /** Max scalar values we accept in a peer-supplied device name. */
        const val PEER_NAME_MAX_CHARS: Int = 64

        /** Cap on the concurrent in-flight handshake state map. */
        const val MAX_CONCURRENT_PEERS: Int = 8

        /** Wall-clock budget for the dual-approval phase. After this
         *  the orchestrator auto-rejects the pairing on the next
         *  inbound frame so peers cannot pin our state forever. */
        const val APPROVAL_TIMEOUT_MS: Long = 60_000L

        private fun ByteArray.toHex(): String =
            joinToString("") { "%02x".format(it) }

        /** Short prefix only, suitable for diagnostic logs (spec §3.5). */
        private fun ByteArray.toHexPrefix(): String =
            take(4).joinToString("") { "%02x".format(it) } + "…"

        /**
         * Sanitize a device name before it crosses the wire or lands in
         * trust storage. Mirrors the Rust-side `sanitize_peer_name`:
         *   * strip C0 / DEL / C1 control chars;
         *   * strip Unicode bidi-override (U+202A..U+202E, U+2066..U+2069);
         *   * cap to PEER_NAME_MAX_CHARS code points;
         *   * trim surrounding whitespace.
         */
        fun sanitizePeerName(input: String): String {
            val out = StringBuilder()
            var count = 0
            val iter = input.codePointIterator()
            while (iter.hasNext() && count < PEER_NAME_MAX_CHARS) {
                val cp = iter.next()
                if (cp < 0x20 || cp == 0x7F) continue
                if (cp in 0x80..0x9F) continue
                if (cp in 0x202A..0x202E) continue
                if (cp in 0x2066..0x2069) continue
                out.appendCodePoint(cp)
                count += 1
            }
            return out.toString().trim()
        }

        private fun String.codePointIterator(): IntIterator = object : IntIterator() {
            private var idx = 0
            override fun hasNext(): Boolean = idx < this@codePointIterator.length
            override fun nextInt(): Int {
                val cp = this@codePointIterator.codePointAt(idx)
                idx += Character.charCount(cp)
                return cp
            }
        }
    }
}
