package com.vortex.a3.core.ble

import org.junit.jupiter.api.Test
import kotlin.test.assertEquals

/**
 * Drift tripwire for the BINDING CONTRACT (spec §10.1).
 *
 * These literals are duplicated in `l3/daemon/src/core/ble/mod.rs`
 * (`uuid_contract_pins`). If you change a UUID here, this test fails — a
 * deliberate nudge to update the spec and the Rust mirror in the same
 * change. The values are the on-air contract: two builds with mismatched
 * UUIDs never discover each other.
 */
class ConstantsContractTest {
    @Test
    fun uuidContractPins() {
        assertEquals(0x01.toByte(), Ble.V1_VERSION)
        assertEquals("53ffc983-45f6-4891-a826-094ac749c063", Ble.VORTEX_SERVICE_UUID.toString())
        assertEquals("b68f4442-adad-4d7c-a944-95c57b5094c7", Ble.PAIRING_CONTROL_UUID.toString())
        assertEquals("bd2e76f0-d216-4f3b-9704-70cc272c3072", Ble.RECONNECT_CONTROL_UUID.toString())
        assertEquals("e78510a3-b39e-459f-8957-864cdb301282", Ble.CAPABILITY_UUID.toString())
        assertEquals("c2e1c97f-3a4b-4d7e-9f0c-1e6a8b3d9c5f", Ble.AUDIO_SIGNAL_UUID.toString())
    }
}
