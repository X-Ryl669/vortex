package com.vortex.a3.core.ble

import org.junit.jupiter.api.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class AdvPayloadTest {

    @Test
    fun `pairable round trip`() {
        val id = byteArrayOf(0x01, 0x23, 0x45, 0x67, 0x89.toByte(), 0xab.toByte(), 0xcd.toByte(), 0xef.toByte())
        val p = AdvPayload.pairable(id)
        val encoded = p.encode()
        assertEquals(10, encoded.size)
        assertEquals(0x01.toByte(), encoded[0])
        assertEquals(0x01.toByte(), encoded[1])
        assertContentEquals(id, encoded.copyOfRange(2, 10))

        val decoded = AdvPayload.decode(encoded).getOrThrow()
        assertEquals(p, decoded)
        assertTrue(decoded.flags.isPairable)
        assertFalse(decoded.flags.isTrustedPresence)
    }

    @Test
    fun `trusted presence round trip`() {
        val token = ByteArray(8) { 0xAA.toByte() }
        val p = AdvPayload.trustedPresence(token)
        val encoded = p.encode()
        assertEquals(0x02.toByte(), encoded[1])

        val decoded = AdvPayload.decode(encoded).getOrThrow()
        assertTrue(decoded.flags.isTrustedPresence)
        assertFalse(decoded.flags.isPairable)
    }

    @Test
    fun `rejects wrong length`() {
        assertTrue(AdvPayload.decode(ByteArray(9)).isFailure)
        assertTrue(AdvPayload.decode(ByteArray(11)).isFailure)
    }

    @Test
    fun `rejects wrong version`() {
        val bytes = AdvPayload.pairable(ByteArray(8)).encode()
        bytes[0] = 0x02
        assertTrue(AdvPayload.decode(bytes).isFailure)
    }

    @Test
    fun `rejects reserved bits set`() {
        val bytes = AdvPayload.pairable(ByteArray(8)).encode()
        bytes[1] = 0x05 // bit0 + bit2 (reserved)
        assertTrue(AdvPayload.decode(bytes).isFailure)
    }

    @Test
    fun `rejects both modes set`() {
        val bytes = AdvPayload.pairable(ByteArray(8)).encode()
        bytes[1] = 0x03 // bit0 + bit1 — mutually exclusive
        assertTrue(AdvPayload.decode(bytes).isFailure)
    }

    @Test
    fun `rejects no mode set`() {
        val bytes = AdvPayload.pairable(ByteArray(8)).encode()
        bytes[1] = 0x00
        assertTrue(AdvPayload.decode(bytes).isFailure)
    }

    /**
     * Cross-platform parity: bytes encoded by Kotlin AdvPayload MUST equal
     * the layout that the Rust l3 codec produces and accepts.
     */
    @Test
    fun `parity with l3 wire layout`() {
        val id = byteArrayOf(0x01, 0x23, 0x45, 0x67, 0x89.toByte(), 0xab.toByte(), 0xcd.toByte(), 0xef.toByte())
        val encoded = AdvPayload.pairable(id).encode()
        // Expected exact bytes (cf. l3 ble::AdvPayload::pairable round-trip test).
        val expected = byteArrayOf(
            0x01, 0x01,
            0x01, 0x23, 0x45, 0x67, 0x89.toByte(), 0xab.toByte(), 0xcd.toByte(), 0xef.toByte(),
        )
        assertContentEquals(expected, encoded)
    }
}
