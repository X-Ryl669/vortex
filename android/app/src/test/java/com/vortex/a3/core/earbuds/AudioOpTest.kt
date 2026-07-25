package com.vortex.a3.core.earbuds

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * Wire interop with `l3/daemon/src/core/audio_op.rs`. Both sides MUST
 * agree on the exact JSON shape; mismatches surface as silent drops
 * (frame fails to decode → orchestrator times out → bad UX).
 */
class AudioOpTest {

    @Test
    fun `roundtrip request`() {
        val f = AudioOpFrame(
            nonce = 42L,
            op = AudioOp.Request,
            mac = "AC:47:1B:25:71:C2",
            ts = 1_700_000_000L,
        )
        val bytes = f.toJsonBytes()
        val back = AudioOpFrame.fromJsonBytes(bytes)
        assertNotNull(back)
        assertEquals(42L, back!!.nonce)
        assertTrue(back.op is AudioOp.Request)
        assertEquals(f.mac, back.mac)
        assertEquals(f.ts, back.ts)
    }

    @Test
    fun `roundtrip reject with reason`() {
        val f = AudioOpFrame(
            nonce = 100L,
            op = AudioOp.Reject(RejectReason.InCall),
            mac = "AA:BB:CC:DD:EE:FF",
            ts = 0L,
        )
        val back = AudioOpFrame.fromJsonBytes(f.toJsonBytes())
        assertNotNull(back)
        val op = back!!.op
        assertTrue(op is AudioOp.Reject)
        assertEquals(RejectReason.InCall, (op as AudioOp.Reject).reason)
    }

    @Test
    fun `roundtrip failed with stage`() {
        val f = AudioOpFrame(
            nonce = 7L,
            op = AudioOp.Failed(Stage.Connect, "A2DP profile refused"),
            mac = "AA:BB:CC:DD:EE:FF",
            ts = 1L,
        )
        val back = AudioOpFrame.fromJsonBytes(f.toJsonBytes())
        assertNotNull(back)
        val op = back!!.op
        assertTrue(op is AudioOp.Failed)
        val failed = op as AudioOp.Failed
        assertEquals(Stage.Connect, failed.stage)
        assertEquals("A2DP profile refused", failed.message)
    }

    @Test
    fun `cross-language interop — accepts rust-side format`() {
        // Byte-for-byte the JSON the Rust side emits (serde tag=kind +
        // snake_case enum). If this test fails, the wire format
        // diverged.
        val rustJson = """{"nonce":1,"op":{"kind":"reject","reason":"recent_switch"},"mac":"a","ts":0}"""
            .toByteArray(Charsets.UTF_8)
        val f = AudioOpFrame.fromJsonBytes(rustJson)
        assertNotNull(f)
        val op = f!!.op
        assertTrue(op is AudioOp.Reject)
        assertEquals(RejectReason.RecentSwitch, (op as AudioOp.Reject).reason)
    }

    @Test
    fun `unknown reject reason fails decode`() {
        // Rust side also errors on unknown reason; matching V1 behaviour.
        val bad = """{"nonce":1,"op":{"kind":"reject","reason":"future_reason"},"mac":"x","ts":0}"""
            .toByteArray(Charsets.UTF_8)
        val f = AudioOpFrame.fromJsonBytes(bad)
        assertNull(f, "unknown reason should reject the frame in V1")
    }

    @Test
    fun `unknown op kind fails decode`() {
        val bad = """{"nonce":1,"op":{"kind":"future_op"},"mac":"x","ts":0}"""
            .toByteArray(Charsets.UTF_8)
        assertNull(AudioOpFrame.fromJsonBytes(bad))
    }

    @Test
    fun `missing nonce fails decode`() {
        val bad = """{"op":{"kind":"request"},"mac":"x","ts":0}"""
            .toByteArray(Charsets.UTF_8)
        assertNull(AudioOpFrame.fromJsonBytes(bad))
    }

    @Test
    fun `blank mac fails decode`() {
        val bad = """{"nonce":1,"op":{"kind":"request"},"mac":"","ts":0}"""
            .toByteArray(Charsets.UTF_8)
        assertNull(AudioOpFrame.fromJsonBytes(bad))
    }
}
