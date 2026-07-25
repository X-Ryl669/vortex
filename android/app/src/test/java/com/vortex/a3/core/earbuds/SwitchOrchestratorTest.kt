package com.vortex.a3.core.earbuds

import com.vortex.a3.core.storage.PeerStore
import com.vortex.a3.core.storage.TrustedPeer
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.atomic.AtomicLong

/**
 * State-machine-only tests for [SwitchOrchestrator]. The
 * [AudioDeviceController] is faked so we exercise the CAS transitions
 * and frame dispatch without touching a real Bluetooth stack.
 */
class SwitchOrchestratorTest {

    private val peerPub = ByteArray(32) { (it + 1).toByte() }
    private val mac = "AC:47:1B:25:71:C2"

    @Test
    fun `happy path - initiator request reaches Idle after connect`() = runBlocking {
        val ctrl = FakeController(connectOk = true)
        val store = FakePeerStore()
        val sent = ConcurrentLinkedQueue<AudioOpFrame>()
        val orch = SwitchOrchestrator(
            controller = ctrl,
            peerStore = store,
            sender = { _, f -> sent.add(f); Result.success(Unit) },
        )

        assertTrue(orch.request(peerPub, mac))
        // Fast-switch: connect attempts fire immediately, no need to
        // wait for an Approve/Released round-trip first.
        waitFor { sent.any { it.op is AudioOp.Done } }
        assertEquals(SwitchState.Idle, orch.state.value)
        assertTrue(ctrl.connectCount.get() >= 1L, "controller.connect should have fired at least once")
    }

    @Test
    fun `peer Reject ends the initiator flow with Failed then Idle`() = runBlocking {
        // Fast-switch protocol: the initiator starts BT connect retries
        // immediately. We use connectOk=false so the loop is still
        // running (in retry pauses) when the Reject arrives.
        val ctrl = FakeController(connectOk = false)
        val store = FakePeerStore()
        val sent = ConcurrentLinkedQueue<AudioOpFrame>()
        val orch = SwitchOrchestrator(ctrl, store, sender = { _, f -> sent.add(f); Result.success(Unit) })

        orch.request(peerPub, mac)
        waitForFrameKind(sent, "request")

        orch.onIncoming(peerPub, AudioOpFrame(2L, AudioOp.Reject(RejectReason.InCall), mac, 0L))

        // Failed state surfaces after the Reject is processed.
        waitFor { orch.state.value is SwitchState.Failed }
        val s = orch.state.value as SwitchState.Failed
        assertTrue(s.reason.contains("in_call", ignoreCase = true) ||
                   s.reason.contains("InCall", ignoreCase = true))
    }

    @Test
    fun `replay rejection - duplicate nonce on Released is dropped`() = runBlocking {
        val ctrl = FakeController(connectOk = true)
        val store = FakePeerStore()
        val sent = ConcurrentLinkedQueue<AudioOpFrame>()
        val orch = SwitchOrchestrator(ctrl, store, sender = { _, f -> sent.add(f); Result.success(Unit) })

        orch.request(peerPub, mac)
        waitForFrameKind(sent, "request")
        waitFor { sent.any { it.op is AudioOp.Done } }

        // Duplicate Released with stale nonce — must be dropped silently
        // by the replay-rejection check in onIncoming, not even reach
        // the state machine.
        orch.onIncoming(peerPub, AudioOpFrame(0L, AudioOp.Released, mac, 0L))
        delay(50)
        // State is Idle (we already finished); the duplicate didn't
        // accidentally retransition us.
        assertEquals(SwitchState.Idle, orch.state.value)
    }

    @Test
    fun `responder flow - incoming Request emits Approve then Released`() = runBlocking {
        val ctrl = FakeController(connectOk = true)
        val store = FakePeerStore()
        val sent = ConcurrentLinkedQueue<AudioOpFrame>()
        val orch = SwitchOrchestrator(ctrl, store, sender = { _, f -> sent.add(f); Result.success(Unit) })

        // Peer asks us (responder side).
        orch.onIncoming(peerPub, AudioOpFrame(10L, AudioOp.Request, mac, 0L))

        waitFor { sent.any { it.op is AudioOp.Released } }
        // Order: Approve before Released.
        val ops = sent.toList().map { it.op::class.simpleName }
        val approveIdx = ops.indexOf("Approve")
        val releasedIdx = ops.indexOf("Released")
        assertTrue(approveIdx >= 0)
        assertTrue(releasedIdx > approveIdx, "Approve must come before Released")
        assertEquals(1L, ctrl.disconnectCount.get(), "responder must disconnect once")
    }

    @Test
    fun `concurrent initiator and responder - second request rejected with Busy`() = runBlocking {
        // Owner-vote: only one flow at a time. While initiator is in
        // flight, an incoming Request must be rejected with Busy.
        // We need the initiator's connect retries to still be running
        // when the peer Request arrives — connectOk=false keeps the
        // loop in retry pauses for the test window.
        val ctrl = FakeController(connectOk = false)
        val store = FakePeerStore()
        val sent = ConcurrentLinkedQueue<AudioOpFrame>()
        val orch = SwitchOrchestrator(ctrl, store, sender = { _, f -> sent.add(f); Result.success(Unit) })

        orch.request(peerPub, mac)
        waitForFrameKind(sent, "request")

        // Peer ALSO asks at the same time. Should get Busy.
        orch.onIncoming(peerPub, AudioOpFrame(20L, AudioOp.Request, mac, 0L))

        waitFor { sent.any { it.op is AudioOp.Reject } }
        val reject = sent.first { it.op is AudioOp.Reject }.op as AudioOp.Reject
        assertEquals(RejectReason.Busy, reject.reason)
    }

    @Test
    fun `acceptance Reject - external policy denies incoming Request`() = runBlocking {
        // E.g. Phase 2: a call is active → reject with InCall.
        val ctrl = FakeController(connectOk = true)
        val store = FakePeerStore()
        val sent = ConcurrentLinkedQueue<AudioOpFrame>()
        val orch = SwitchOrchestrator(
            ctrl, store,
            sender = { _, f -> sent.add(f); Result.success(Unit) },
            acceptanceProvider = { SwitchOrchestrator.Acceptance.Reject(RejectReason.InCall) },
        )

        orch.onIncoming(peerPub, AudioOpFrame(30L, AudioOp.Request, mac, 0L))

        waitFor { sent.any { it.op is AudioOp.Reject } }
        val r = sent.first { it.op is AudioOp.Reject }.op as AudioOp.Reject
        assertEquals(RejectReason.InCall, r.reason)
        assertEquals(0L, ctrl.disconnectCount.get(), "must NOT disconnect when rejected")
    }

    // ---- test infra ----

    private suspend fun waitFor(timeoutMs: Long = 1500, predicate: () -> Boolean) {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (System.currentTimeMillis() < deadline) {
            if (predicate()) return
            delay(20)
        }
        throw AssertionError("predicate not satisfied within ${timeoutMs}ms")
    }

    private suspend fun waitForFrameKind(q: ConcurrentLinkedQueue<AudioOpFrame>, kind: String) {
        waitFor { q.any { it.op::class.simpleName?.equals(kind, ignoreCase = true) == true } }
    }

    private class FakeController(
        private val connectOk: Boolean,
        private val slowDisconnect: Long = 0L,
    ) : AudioDeviceHandle {
        val connectCount = AtomicLong(0)
        val disconnectCount = AtomicLong(0)
        override fun prewarm() { /* noop */ }
        override fun isConnected(mac: String): Boolean = false
        override fun invalidate(mac: String) { /* noop */ }
        override fun close() { /* noop */ }
        override suspend fun connect(mac: String): Result<Unit> {
            connectCount.incrementAndGet()
            return if (connectOk) Result.success(Unit)
                else Result.failure(IllegalStateException("fake connect refused"))
        }
        override suspend fun disconnect(mac: String): Result<Unit> {
            if (slowDisconnect > 0) delay(slowDisconnect)
            disconnectCount.incrementAndGet()
            return Result.success(Unit)
        }
    }

    private class FakePeerStore : PeerStore {
        private val outN = AtomicLong(0)
        private val inN = AtomicLong(0)
        override fun save(peer: TrustedPeer) {}
        override fun load(peerStaticPub: ByteArray): TrustedPeer? = null
        override fun list(): List<TrustedPeer> = emptyList()
        override fun forget(peerStaticPub: ByteArray) {}
        override fun nextAudioOutNonce(peerStaticPub: ByteArray): Long = outN.incrementAndGet()
        override fun loadAudioInNonce(peerStaticPub: ByteArray): Long = inN.get()
        override fun commitAudioInNonce(peerStaticPub: ByteArray, nonce: Long) {
            inN.updateAndGet { maxOf(it, nonce) }
        }
    }
}
