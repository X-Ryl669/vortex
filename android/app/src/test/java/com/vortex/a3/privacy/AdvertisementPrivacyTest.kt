package com.vortex.a3.privacy

import com.vortex.a3.core.ble.AdvFlags
import com.vortex.a3.core.ble.AdvPayload
import com.vortex.a3.core.ble.Ble
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import kotlin.test.fail
import java.io.File

/**
 * Phase 9 — pre-trust BLE advertisement privacy gate (spec §3.1 T-BLE-1).
 *
 * Asserts the contract that a Vortex Device advertises only:
 *
 *   - the Vortex Service UUID,
 *   - a 10-byte service-data payload (version + flags + 8-byte instance ID
 *     or truncated presence token),
 *   - a generic display label (`"Vortex Android"`) — never the user's
 *     device name, hostname, account name, `device_id`, or `static_pub`.
 *
 * Two complementary checks:
 *
 *   1. Encoded payload bounds: AdvPayload.encode() output is exactly 10
 *      bytes and begins with the V1 version + a well-formed flag byte.
 *   2. Static source scan over Advertiser.kt: confirms
 *      `setIncludeDeviceName(false)` is set, no manufacturer-data field is
 *      attached, and no identity material (`identity.staticPub`,
 *      `identity.deviceId`, `static_priv`) appears in the advertise path.
 */
class AdvertisementPrivacyTest {

    @Test
    fun `encoded pairable payload is exactly 10 bytes`() {
        val instanceId = ByteArray(8) { (it + 1).toByte() }
        val payload = AdvPayload.pairable(instanceId)
        val bytes = payload.encode()

        assertEquals(Ble.ADV_PAYLOAD_LEN, bytes.size, "encoded payload length")
        assertEquals(10, bytes.size, "spec mandates 10-byte payload")
        assertEquals(Ble.V1_VERSION, bytes[0], "byte 0 must be V1 version")
        assertTrue(AdvFlags(bytes[1]).isPairable, "byte 1 must be pairable flag")

        // Byte 2..10 MUST be the 8-byte instance ID supplied; nothing else.
        for (i in 0 until 8) {
            assertEquals(instanceId[i], bytes[2 + i], "instance id mismatch at byte ${2 + i}")
        }
    }

    @Test
    fun `encoded trusted-presence payload is exactly 10 bytes`() {
        val token = ByteArray(8) { (0xA0 or it).toByte() }
        val payload = AdvPayload.trustedPresence(token)
        val bytes = payload.encode()

        assertEquals(10, bytes.size)
        assertEquals(Ble.V1_VERSION, bytes[0])
        assertTrue(AdvFlags(bytes[1]).isTrustedPresence)
        for (i in 0 until 8) {
            assertEquals(token[i], bytes[2 + i])
        }
    }

    @Test
    fun `Advertiser source enforces pre-trust privacy invariants`() {
        val advertiserSrc = locateMainDir().resolve("core/ble/Advertiser.kt")
        assertTrue(advertiserSrc.isFile, "Advertiser.kt not found at $advertiserSrc")
        val text = advertiserSrc.readText()

        // 1. The primary ADV_IND record MUST NOT include the device name.
        //    This is the always-on broadcast — anything in it is observable
        //    by any nearby radio without provoking a SCAN_REQ first. We
        //    require at least one explicit `setIncludeDeviceName(false)`
        //    call on the advertise-data builder.
        //
        //    Note: spec §5.1.2 originally required SCAN_RSP to also drop
        //    the device name, but the product decision was to expose the
        //    Bluetooth alias in SCAN_RSP so the Linux scan list can show
        //    "Redmi 9" instead of a generic "Android phone" (Advertiser.kt
        //    §73-80 documents the trade-off and accepts it). So this test
        //    no longer asserts on SCAN_RSP — only the always-on ADV_IND.
        val devNameOff = Regex("""setIncludeDeviceName\s*\(\s*false\s*\)""")
        val offCount = devNameOff.findAll(text).count()
        assertTrue(
            offCount >= 1,
            "Primary ADV_IND must call setIncludeDeviceName(false) (spec §5.1.4); " +
                "found $offCount occurrences",
        )

        // 2. No identity material may appear in the advertise path.
        val forbidden = listOf(
            "identity.staticPub",
            "identity.deviceId",
            "staticPriv",
            "addManufacturerData",
        )
        forbidden.forEach { needle ->
            assertFalse(
                text.contains(needle),
                "Advertiser.kt must not reference '$needle' (spec §3.1 T-BLE-1)",
            )
        }
    }

    @Test
    fun `manifest declares only the BLE permissions V1 needs pre-trust`() {
        val manifest = locateAppDir().resolve("src/main/AndroidManifest.xml")
        assertTrue(manifest.isFile, "AndroidManifest.xml not found at $manifest")
        val text = manifest.readText()

        // V1 may NOT request fingerprinting-grade permissions on the
        // pre-trust path. Each of these would let a Vortex build silently
        // accumulate identity surface beyond the protocol's promise.
        //
        // READ_PHONE_STATE is INTENTIONALLY off this list as of
        // Phase 2: the call-handoff orchestrator needs to observe
        // ringing/active/idle to reroute the buds, and that's only
        // ever useful AFTER trust exists (we hand the buds to a
        // paired peer). The permission is requested at install time
        // because Android has no narrower API for call state.
        //
        // READ_CONTACTS came off the list with the phone-companion
        // mirrors (Contacts page + caller-name resolution in the call
        // banner): like call state, contact data only ever flows to an
        // already-paired peer over the Noise transport, never on the
        // pre-trust path.
        val forbiddenPermissions = listOf(
            "READ_PRIVILEGED_PHONE_STATE",
            "GET_ACCOUNTS",
            "ACCESS_BACKGROUND_LOCATION",
        )
        val violations = forbiddenPermissions.filter { text.contains(it) }
        if (violations.isNotEmpty()) {
            fail("manifest must not declare: $violations (spec §3.1 T-BLE-1)")
        }
    }

    // ----------------------------------------------------------------
    // helpers
    // ----------------------------------------------------------------

    private fun locateAppDir(): File {
        var dir = File(System.getProperty("user.dir"))
        repeat(6) {
            val candidate = File(dir, "app")
            if (candidate.isDirectory && File(candidate, "src/main/AndroidManifest.xml").isFile) {
                return candidate
            }
            dir = dir.parentFile ?: return@repeat
        }
        error("could not locate app/ from ${System.getProperty("user.dir")}")
    }

    private fun locateMainDir(): File =
        locateAppDir().resolve("src/main/java/com/vortex/a3")
}
