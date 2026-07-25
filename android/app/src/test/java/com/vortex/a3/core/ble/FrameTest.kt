package com.vortex.a3.core.ble

import org.junit.jupiter.api.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class FrameTest {

    @Test
    fun `empty payload round trip matches spec`() {
        val f = Frame(FrameType.PAIRING_APPROVAL, FrameSub.ECHO_REQUEST, ByteArray(0))
        val bytes = f.encode()
        assertContentEquals(byteArrayOf(0x11, 0x01, 0x00, 0x00), bytes)
        assertEquals(f, Frame.decode(bytes).getOrThrow())
    }

    @Test
    fun `small payload round trip matches l3 wire`() {
        val f = Frame(
            FrameType.TRANSPORT_KEEPALIVE,
            FrameSub.ECHO_REQUEST,
            byteArrayOf(0xAA.toByte(), 0xBB.toByte(), 0xCC.toByte()),
        )
        val bytes = f.encode()
        // Same bytes as l3 frame::Frame::echo_request(vec![0xAA, 0xBB, 0xCC])
        // produces in core/ble/frame.rs.
        assertContentEquals(
            byteArrayOf(0x30, 0x01, 0x00, 0x03, 0xAA.toByte(), 0xBB.toByte(), 0xCC.toByte()),
            bytes,
        )
        assertEquals(f, Frame.decode(bytes).getOrThrow())
    }

    @Test
    fun `rejects short header`() {
        assertTrue(Frame.decode(byteArrayOf(0x10, 0x01)).isFailure)
    }

    @Test
    fun `rejects length mismatch`() {
        // Header says length=5, payload only 3 bytes.
        val bytes = byteArrayOf(0x10, 0x01, 0x00, 0x05, 0xAA.toByte(), 0xBB.toByte(), 0xCC.toByte())
        assertTrue(Frame.decode(bytes).isFailure)
    }
}
