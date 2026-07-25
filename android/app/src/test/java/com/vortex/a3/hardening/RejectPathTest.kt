package com.vortex.a3.hardening

import android.bluetooth.BluetoothDevice
import com.southernstorm.noise.protocol.CipherState
import com.southernstorm.noise.protocol.CipherStatePair
import com.southernstorm.noise.protocol.HandshakeState
import com.southernstorm.noise.protocol.Noise
import com.vortex.a3.core.ble.Frame
import com.vortex.a3.core.ble.FrameType
import com.vortex.a3.core.crypto.NoiseRunner
import com.vortex.a3.core.crypto.X25519
import com.vortex.a3.core.identity.IdentityRecord
import com.vortex.a3.core.identity.Platform
import com.vortex.a3.core.pairing.PairingOrchestrator
import io.mockk.every
import io.mockk.mockk
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import java.util.concurrent.CopyOnWriteArrayList

/**
 * Phase 10 — wrong-SAS / peer-rejection state hygiene
 * (the build plan Phase 10, threat model §3.2 T-PAIR-3).
 *
 * Drives [PairingOrchestrator] (the Android XX responder) through a full
 * Noise XX handshake using a real noise-java initiator on the same JVM.
 * After XX completes, the test sends a `PAIRING_APPROVAL(reject)` frame
 * and asserts:
 *
 *   - the orchestrator emits an outcome with `state = PeerRejected`,
 *   - the orchestrator does NOT emit `BothApproved`,
 *   - `peerPrs(addr)` returns null (no trust derived → no PRS to persist),
 *   - subsequent stray approvals on the same address do not flip state to
 *     `BothApproved` (no zombie state).
 */
class RejectPathTest {

    private val testAddress = "AA:BB:CC:DD:EE:01"

    @Test
    fun `peer reject does not derive PRS or persist trust`() {
        val orch = newOrchestratorAutoApproveOff()
        val device = newFakeDevice(testAddress)
        val outcomes = CopyOnWriteArrayList<PairingOrchestrator.HandshakeOutcome>()
        orch.addListener { outcomes.add(it) }

        val initiator = newInitiator()

        // -------- XX: msg1 (initiator → responder) --------
        val msg1Bytes = ByteArray(Noise.MAX_PACKET_LEN)
        val n1 = initiator.writeMessage(msg1Bytes, 0, null, 0, 0)
        val msg2Frame = orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_HANDSHAKE, PairingOrchestrator.HANDSHAKE_MSG1, msg1Bytes.copyOf(n1)),
        )
        assertNotNull(msg2Frame, "responder must emit msg2 in reply to msg1")
        assertEquals(FrameType.PAIRING_HANDSHAKE, msg2Frame!!.type)
        assertEquals(PairingOrchestrator.HANDSHAKE_MSG2, msg2Frame.sub)

        // -------- XX: msg2 (responder → initiator) --------
        val readBuf = ByteArray(Noise.MAX_PACKET_LEN)
        initiator.readMessage(msg2Frame.payload, 0, msg2Frame.payload.size, readBuf, 0)

        // -------- XX: msg3 (initiator → responder) --------
        val msg3Bytes = ByteArray(Noise.MAX_PACKET_LEN)
        val n3 = initiator.writeMessage(msg3Bytes, 0, null, 0, 0)
        val maybeApproval = orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_HANDSHAKE, PairingOrchestrator.HANDSHAKE_MSG3, msg3Bytes.copyOf(n3)),
        )
        // autoApprove=false in this test → no auto outbound approval frame.
        assertNull(maybeApproval, "auto-approve disabled — no approval frame should be emitted")

        // The orchestrator should have emitted XxComplete by now.
        val xxOutcome = outcomes.firstOrNull { it.state == PairingOrchestrator.PhaseState.XxComplete }
        assertNotNull(xxOutcome, "expected XxComplete outcome after msg3")

        // The post-handshake APPROVE / REJECT frames are now AEAD-wrapped
        // by Noise transport mode — match the production wire format.
        val pair = initiator.split()

        // -------- Peer rejects --------
        orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_APPROVAL, PairingOrchestrator.APPROVAL_REJECT, sealEmpty(pair.sender)),
        )

        // Assertion 1: orchestrator emitted PeerRejected, never BothApproved.
        val rejected = outcomes.firstOrNull { it.state == PairingOrchestrator.PhaseState.PeerRejected }
        assertNotNull(rejected, "expected PeerRejected outcome after reject frame")
        val approved = outcomes.firstOrNull { it.state == PairingOrchestrator.PhaseState.BothApproved }
        assertNull(approved, "BothApproved must NOT be emitted after a reject frame")

        // Assertion 2: no PRS derived for this peer (so MainActivity's
        // listener cannot accidentally persist trust).
        assertNull(orch.peerPrs(testAddress), "peerPrs must be null after reject")

        // Assertion 3: a stray approve frame after rejection must not
        // flip the state to BothApproved (no zombie state).
        orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_APPROVAL, PairingOrchestrator.APPROVAL_APPROVE, sealEmpty(pair.sender)),
        )
        val approvedAfterStray = outcomes.firstOrNull {
            it.state == PairingOrchestrator.PhaseState.BothApproved
        }
        assertNull(
            approvedAfterStray,
            "stray approve frame after reject must not yield BothApproved",
        )
        assertNull(orch.peerPrs(testAddress), "peerPrs must remain null after stray approve")
    }

    @Test
    fun `dual approve derives PRS only after BOTH sides approve`() {
        // Counter-test for the reject path: BothApproved fires only when
        // BOTH local user AND peer have approved. This guards spec §3.2
        // T-PAIR-3 — peer's approval alone must NOT auto-trust.
        val orch = newOrchestratorAutoApproveOff()
        val device = newFakeDevice(testAddress)
        val outcomes = CopyOnWriteArrayList<PairingOrchestrator.HandshakeOutcome>()
        orch.addListener { outcomes.add(it) }
        val initiator = newInitiator()

        val msg1Bytes = ByteArray(Noise.MAX_PACKET_LEN)
        val n1 = initiator.writeMessage(msg1Bytes, 0, null, 0, 0)
        val msg2Frame = orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_HANDSHAKE, PairingOrchestrator.HANDSHAKE_MSG1, msg1Bytes.copyOf(n1)),
        )!!
        val readBuf = ByteArray(Noise.MAX_PACKET_LEN)
        initiator.readMessage(msg2Frame.payload, 0, msg2Frame.payload.size, readBuf, 0)
        val msg3Bytes = ByteArray(Noise.MAX_PACKET_LEN)
        val n3 = initiator.writeMessage(msg3Bytes, 0, null, 0, 0)
        orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_HANDSHAKE, PairingOrchestrator.HANDSHAKE_MSG3, msg3Bytes.copyOf(n3)),
        )

        val pair = initiator.split()

        // Peer approves FIRST (the realistic ordering — Linux initiator
        // sends APPROVAL before Android responder's user has had time
        // to tap the SAS dialog).
        orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_APPROVAL, PairingOrchestrator.APPROVAL_APPROVE, sealEmpty(pair.sender)),
        )

        // CRITICAL: peer-approval-alone must NOT yield BothApproved or
        // a derived PRS — the local user has not decided yet.
        assertNull(
            outcomes.firstOrNull { it.state == PairingOrchestrator.PhaseState.BothApproved },
            "peer approval alone must not yield BothApproved",
        )
        assertNull(orch.peerPrs(testAddress), "peerPrs must be null until local user approves")

        // Now local user approves via the UI.
        val approveFrame = orch.userApprove(device)
        assertNotNull(approveFrame, "userApprove must return APPROVE frame in AwaitingApprovals state")
        assertEquals(FrameType.PAIRING_APPROVAL, approveFrame!!.type)
        assertEquals(PairingOrchestrator.APPROVAL_APPROVE, approveFrame.sub)

        val approved = outcomes.firstOrNull {
            it.state == PairingOrchestrator.PhaseState.BothApproved
        }
        assertNotNull(approved, "BothApproved must fire after local user approves")
        val prs = orch.peerPrs(testAddress)
        assertNotNull(prs, "PRS must be derived after both-approve")
        assertTrue(prs!!.size == 32, "PRS must be 32 bytes; was ${prs.size}")
    }

    @Test
    fun `forged APPROVE with bad AEAD must not advance peerDecision`() {
        // Bug 1 (bugs.md 2026-05-14): handlePeerApproval previously assigned
        // s.peerDecision from sub BEFORE AEAD-verifying the frame. A hostile
        // peer could send sub=APPROVE with garbage payload; AEAD decrypt would
        // fail, the function returned null, but s.peerDecision was already
        // set to Approve. A later local userApprove would then yield
        // BothApproved — a one-sided trust breach.
        //
        // After the fix, peerDecision is committed only after authenticated
        // decrypt succeeds. This test drives the bad-frame path and asserts
        // BothApproved never fires and no PRS is derived.
        val orch = newOrchestratorAutoApproveOff()
        val device = newFakeDevice(testAddress)
        val outcomes = CopyOnWriteArrayList<PairingOrchestrator.HandshakeOutcome>()
        orch.addListener { outcomes.add(it) }
        val initiator = newInitiator()

        val msg1Bytes = ByteArray(Noise.MAX_PACKET_LEN)
        val n1 = initiator.writeMessage(msg1Bytes, 0, null, 0, 0)
        val msg2Frame = orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_HANDSHAKE, PairingOrchestrator.HANDSHAKE_MSG1, msg1Bytes.copyOf(n1)),
        )!!
        val readBuf = ByteArray(Noise.MAX_PACKET_LEN)
        initiator.readMessage(msg2Frame.payload, 0, msg2Frame.payload.size, readBuf, 0)
        val msg3Bytes = ByteArray(Noise.MAX_PACKET_LEN)
        val n3 = initiator.writeMessage(msg3Bytes, 0, null, 0, 0)
        orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_HANDSHAKE, PairingOrchestrator.HANDSHAKE_MSG3, msg3Bytes.copyOf(n3)),
        )
        // Split is needed to keep the initiator state consistent, but we
        // deliberately do NOT use its CipherState to seal the forged frame.
        initiator.split()

        // Forged APPROVE: sub=APPROVE, payload is random bytes that will
        // fail Poly1305 verification on the responder side.
        val garbage = ByteArray(48) { (it * 7 + 13).toByte() }
        orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_APPROVAL, PairingOrchestrator.APPROVAL_APPROVE, garbage),
        )

        // Forged frame must be silently dropped — no state advance.
        assertNull(
            outcomes.firstOrNull { it.state == PairingOrchestrator.PhaseState.BothApproved },
            "forged APPROVE must not yield BothApproved on its own",
        )

        // Now local user approves. With the bug, this would flip the
        // already-polluted peerDecision into BothApproved. With the fix,
        // peerDecision is still Pending so finalizeIfReady stays in
        // AwaitingApprovals — no BothApproved, no derived PRS.
        orch.userApprove(device)
        assertNull(
            outcomes.firstOrNull { it.state == PairingOrchestrator.PhaseState.BothApproved },
            "local approve must NOT yield BothApproved when peer's only approval was forged",
        )
        assertNull(
            orch.peerPrs(testAddress),
            "peerPrs must be null when peer never AEAD-authenticated their approval",
        )
    }

    @Test
    fun `local user reject yields PeerRejected even if peer approved first`() {
        // Wrong-SAS UX: peer thinks the SAS matched (and sent APPROVE),
        // but the local user sees a different code and taps Reject.
        // EITHER side rejecting must finalize as Rejected.
        val orch = newOrchestratorAutoApproveOff()
        val device = newFakeDevice(testAddress)
        val outcomes = CopyOnWriteArrayList<PairingOrchestrator.HandshakeOutcome>()
        orch.addListener { outcomes.add(it) }
        val initiator = newInitiator()

        val msg1Bytes = ByteArray(Noise.MAX_PACKET_LEN)
        val n1 = initiator.writeMessage(msg1Bytes, 0, null, 0, 0)
        val msg2Frame = orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_HANDSHAKE, PairingOrchestrator.HANDSHAKE_MSG1, msg1Bytes.copyOf(n1)),
        )!!
        val readBuf = ByteArray(Noise.MAX_PACKET_LEN)
        initiator.readMessage(msg2Frame.payload, 0, msg2Frame.payload.size, readBuf, 0)
        val msg3Bytes = ByteArray(Noise.MAX_PACKET_LEN)
        val n3 = initiator.writeMessage(msg3Bytes, 0, null, 0, 0)
        orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_HANDSHAKE, PairingOrchestrator.HANDSHAKE_MSG3, msg3Bytes.copyOf(n3)),
        )

        val pair = initiator.split()

        // Peer approves.
        orch.onPairingControlFrame(
            device,
            Frame(FrameType.PAIRING_APPROVAL, PairingOrchestrator.APPROVAL_APPROVE, sealEmpty(pair.sender)),
        )
        // Local user then rejects (wrong SAS).
        val rejectFrame = orch.userReject(device)
        assertNotNull(rejectFrame, "userReject must return REJECT frame")
        assertEquals(PairingOrchestrator.APPROVAL_REJECT, rejectFrame!!.sub)

        // Final outcome: PeerRejected, no PRS, no zombie BothApproved.
        assertNull(orch.peerPrs(testAddress))
        assertNotNull(
            outcomes.firstOrNull { it.state == PairingOrchestrator.PhaseState.PeerRejected },
            "local reject must yield PeerRejected",
        )
        assertNull(
            outcomes.firstOrNull { it.state == PairingOrchestrator.PhaseState.BothApproved },
            "BothApproved must NOT fire after local reject",
        )
    }

    // --------------------------------------------------------------
    // helpers
    // --------------------------------------------------------------

    private fun newOrchestratorAutoApproveOff(): PairingOrchestrator {
        val identity = synthesizeIdentity()
        return PairingOrchestrator(identity).also { it.autoApprove = false }
    }

    private fun synthesizeIdentity(): IdentityRecord {
        // Deterministic so test failures are reproducible.
        val priv = ByteArray(X25519.PRIV_LEN) { (it + 0x10).toByte() }
        val deviceId = ByteArray(16) { (it + 0xA0).toByte() }
        return IdentityRecord.fromPrivate(
            platform = Platform.Android,
            deviceId = deviceId,
            staticPriv = priv,
            createdAt = 1_700_000_000L,
        )
    }

    private fun newFakeDevice(address: String): BluetoothDevice {
        val device = mockk<BluetoothDevice>()
        every { device.address } returns address
        return device
    }

    private fun newInitiator(): HandshakeState {
        val priv = ByteArray(X25519.PRIV_LEN) { (it + 0x40).toByte() }
        val state = HandshakeState(NoiseRunner.NOISE_XX, HandshakeState.INITIATOR)
        state.setPrologue(NoiseRunner.PROLOGUE_XX, 0, NoiseRunner.PROLOGUE_XX.size)
        state.localKeyPair.setPrivateKey(priv, 0)
        state.start()
        return state
    }

    /**
     * Mirrors the orchestrator's AEAD seal: empty plaintext → 16-byte
     * Poly1305 tag only. Used by the test's simulated initiator side
     * to produce a wire-format-correct PAIRING_APPROVAL payload.
     */
    private fun sealEmpty(cipher: CipherState): ByteArray {
        val out = ByteArray(cipher.macLength)
        val n = cipher.encryptWithAd(null, ByteArray(0), 0, out, 0, 0)
        return out.copyOf(n)
    }
}
