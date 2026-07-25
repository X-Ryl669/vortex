package com.vortex.a3.core.identity

import com.fasterxml.jackson.databind.ObjectMapper
import com.vortex.a3.core.crypto.X25519
import com.vortex.a3.core.storage.InMemoryIdentityStore
import com.vortex.a3.core.storage.loadOrGenerate
import org.junit.jupiter.api.Test
import java.io.File
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

class IdentityTest {

    private val mapper = ObjectMapper()

    private fun locateVectorsDir(): File {
        var dir = File(System.getProperty("user.dir"))
        repeat(6) {
            val candidate = File(dir, "shared/vectors/v1")
            if (candidate.isDirectory) return candidate
            dir = dir.parentFile ?: return@repeat
        }
        error("could not locate shared/vectors/v1")
    }

    private fun hex(s: String): ByteArray {
        require(s.length % 2 == 0)
        return ByteArray(s.length / 2) {
            ((Character.digit(s[it * 2], 16) shl 4) or Character.digit(s[it * 2 + 1], 16)).toByte()
        }
    }

    @Test
    fun `X25519 derivation matches Rust vectors`() {
        val v = mapper.readTree(File(locateVectorsDir(), "x25519.json"))
        v["cases"].forEach { case ->
            val priv = hex(case["private_hex"].asText())
            val pubExpected = hex(case["public_hex"].asText())
            val pubActual = X25519.publicFromPrivate(priv)
            assertContentEquals(
                pubExpected, pubActual,
                "X25519 mismatch for private=${case["private_hex"].asText()}",
            )
        }
    }

    @Test
    fun `RFC 7748 Alice keypair`() {
        val priv = hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
        val pubExpected = hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")
        assertContentEquals(pubExpected, X25519.publicFromPrivate(priv))
    }

    @Test
    fun `IdentityRecord encode is 90 bytes per spec`() {
        val priv = ByteArray(32) { it.toByte() }
        val record = IdentityRecord.fromPrivate(
            platform = Platform.Android,
            deviceId = ByteArray(16),
            staticPriv = priv,
            createdAt = 1_700_000_000L,
        )
        val bytes = record.encode()
        assertEquals(90, bytes.size)
        assertEquals(IDENTITY_VERSION, bytes[0])
        assertEquals(0x01.toByte(), bytes[89]) // platform = Android
    }

    @Test
    fun `Generated identities differ`() {
        val a = IdentityRecord.generate(Platform.Android)
        val b = IdentityRecord.generate(Platform.Android)
        assertNotEquals(a.deviceId.toList(), b.deviceId.toList())
        assertNotEquals(a.staticPriv.toList(), b.staticPriv.toList())
        assertNotEquals(a.staticPub.toList(), b.staticPub.toList())
        assertEquals(Platform.Android, a.platform)
    }

    @Test
    fun `InMemoryIdentityStore round trip`() {
        val store = InMemoryIdentityStore()
        assertFalse(store.exists())

        val record = IdentityRecord.generate(Platform.Android)
        store.save(record)

        val loaded = store.load()
        assertTrue(loaded != null)
        assertEquals(record, loaded)
    }

    @Test
    fun `loadOrGenerate creates then reuses`() {
        val store = InMemoryIdentityStore()
        val first = store.loadOrGenerate(Platform.Android)
        val second = store.loadOrGenerate(Platform.Android)
        assertContentEquals(first.deviceId, second.deviceId)
        assertContentEquals(first.staticPub, second.staticPub)
    }
}
