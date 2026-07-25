package com.vortex.a3.core.ble

import java.util.UUID

/** BLE constants per spec §5 and §10. */
object Ble {
    /** V1 protocol version byte. */
    const val V1_VERSION: Byte = 0x01

    // ---- BINDING CONTRACT — wire-level, MUST match the peer ----
    //
    // The GATT UUIDs below are the on-air contract: the Linux side mirrors
    // them byte-for-byte in `l3/daemon/src/core/ble/mod.rs`, and both
    // derive from spec §10.1. Changing any value here is a
    // protocol break — update the spec AND the Rust mirror in the same
    // change, or the two builds stop discovering each other. The
    // `ConstantsContractTest` is the tripwire if a value drifts.
    //
    // (Contrast the timeout / retry constants in `SwitchOrchestrator`,
    // which are local tuning and intentionally differ per side.)

    /** Vortex Service UUID (§10.1). */
    val VORTEX_SERVICE_UUID: UUID = UUID.fromString("53ffc983-45f6-4891-a826-094ac749c063")

    /** Pairing Control characteristic UUID. */
    val PAIRING_CONTROL_UUID: UUID = UUID.fromString("b68f4442-adad-4d7c-a944-95c57b5094c7")

    /** Reconnect Control characteristic UUID. */
    val RECONNECT_CONTROL_UUID: UUID = UUID.fromString("bd2e76f0-d216-4f3b-9704-70cc272c3072")

    /** Capability characteristic UUID. */
    val CAPABILITY_UUID: UUID = UUID.fromString("e78510a3-b39e-459f-8957-864cdb301282")

    /** Audio-signal characteristic (Phase 2). Carries one-byte
     *  AES-GCM-encrypted opcodes for call-handoff and switch events.
     *  Properties: WRITE + NOTIFY. Linux subscribes after pairing;
     *  the phone notifies on call-state changes so handoff goes
     *  through in ~200 ms instead of the LAN heartbeat's 5 s.
     *  Opcode values mirror `SignalBytes` on the Rust side. */
    val AUDIO_SIGNAL_UUID: UUID = UUID.fromString("c2e1c97f-3a4b-4d7e-9f0c-1e6a8b3d9c5f")

    /** V1 service-data payload size (§5.1.3). */
    const val ADV_PAYLOAD_LEN: Int = 10

    /** Generic display name advertised in SCAN_RSP (§5.1.4). */
    const val LOCAL_NAME: String = "Vortex Android"
}
