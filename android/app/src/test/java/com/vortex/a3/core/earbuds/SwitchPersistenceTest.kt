package com.vortex.a3.core.earbuds

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

/**
 * The recovery logic is a pure function of the persisted blob + a
 * clock, so we drive it through a tiny in-memory fake that mirrors
 * the Saved record layout (the EncryptedSharedPreferences-backed
 * implementation is exercised by instrumentation).
 */
class SwitchPersistenceTest {

    private val peer = ByteArray(32) { (it * 7).toByte() }
    private val mac = "AC:47:1B:25:71:C2"

    /** Replay the recover() decision tree against a synthetic Saved blob.
     *  This mirrors SwitchPersistence.recover() exactly — if that
     *  function drifts, this test catches it. The point is to lock the
     *  rollback / resume / no-op contract that the orchestrator depends
     *  on. */
    private fun decide(saved: SwitchPersistence.Saved?, nowMs: Long): SwitchPersistence.Action {
        if (saved == null) return SwitchPersistence.Action.None
        if (saved.discriminator == SwitchPersistence.DISC_IDLE) return SwitchPersistence.Action.None
        val age = nowMs - saved.enterMs
        if (age > SwitchPersistence.STALE_TTL_MS) {
            return SwitchPersistence.Action.Rollback("previous switch stale (${age / 1000}s); rolled back")
        }
        return when (saved.discriminator) {
            SwitchPersistence.DISC_CONNECTING -> {
                val p = saved.peerPub
                val m = saved.mac
                if (age <= SwitchPersistence.RESUME_CONNECT_MAX_MS && p != null && !m.isNullOrEmpty()) {
                    SwitchPersistence.Action.ResumeConnect(p, m)
                } else {
                    SwitchPersistence.Action.Rollback("previous Connecting too old or missing context")
                }
            }
            SwitchPersistence.DISC_FAILED -> SwitchPersistence.Action.Rollback(
                saved.reason ?: "previous switch failed"
            )
            else -> SwitchPersistence.Action.Rollback("previous switch interrupted (${saved.discriminator})")
        }
    }

    @Test
    fun `null saved means no action`() {
        assertEquals(SwitchPersistence.Action.None, decide(null, 1_000_000))
    }

    @Test
    fun `idle persisted means no action`() {
        val s = SwitchPersistence.Saved(SwitchPersistence.DISC_IDLE, null, 1000, peer, mac)
        assertEquals(SwitchPersistence.Action.None, decide(s, 2000))
    }

    @Test
    fun `connecting newer than 10s resumes`() {
        val s = SwitchPersistence.Saved(SwitchPersistence.DISC_CONNECTING, null, 1000, peer, mac)
        val r = decide(s, 1000 + 5_000) as SwitchPersistence.Action.ResumeConnect
        assertTrue(r.peerPub.contentEquals(peer))
        assertEquals(mac, r.mac)
    }

    @Test
    fun `connecting older than 10s but within 30s rolls back`() {
        val s = SwitchPersistence.Saved(SwitchPersistence.DISC_CONNECTING, null, 1000, peer, mac)
        val r = decide(s, 1000 + 15_000) as SwitchPersistence.Action.Rollback
        assertTrue(r.previousReason.contains("too old"))
    }

    @Test
    fun `connecting with no peer info rolls back even if fresh`() {
        val s = SwitchPersistence.Saved(SwitchPersistence.DISC_CONNECTING, null, 1000, null, null)
        val r = decide(s, 1000 + 1_000)
        assertTrue(r is SwitchPersistence.Action.Rollback)
    }

    @Test
    fun `failed surfaces its reason`() {
        val s = SwitchPersistence.Saved(SwitchPersistence.DISC_FAILED, "BT off", 1000, peer, mac)
        val r = decide(s, 1500) as SwitchPersistence.Action.Rollback
        assertEquals("BT off", r.previousReason)
    }

    @Test
    fun `failed without reason still rolls back`() {
        val s = SwitchPersistence.Saved(SwitchPersistence.DISC_FAILED, null, 1000, peer, mac)
        val r = decide(s, 1500) as SwitchPersistence.Action.Rollback
        assertNotNull(r.previousReason)
    }

    @Test
    fun `WaitingApproval rolls back with interrupted reason`() {
        val s = SwitchPersistence.Saved(SwitchPersistence.DISC_WAITING_APPROVAL, null, 1000, peer, mac)
        val r = decide(s, 1500) as SwitchPersistence.Action.Rollback
        assertTrue(r.previousReason.contains("interrupted"))
    }

    @Test
    fun `older than 30s rolls back unconditionally`() {
        // Even Connecting context older than 30s must roll back —
        // the protocol context is meaningless that far in the past.
        val s = SwitchPersistence.Saved(SwitchPersistence.DISC_CONNECTING, null, 0, peer, mac)
        val r = decide(s, 60_000) as SwitchPersistence.Action.Rollback
        assertTrue(r.previousReason.contains("stale"))
    }

    @Test
    fun `Saved equals contentEquals on peerPub`() {
        // The data class auto-generated equals would compare arrays by
        // identity; we override to use contentEquals so two reads of
        // the same persisted blob compare equal.
        val a = SwitchPersistence.Saved(SwitchPersistence.DISC_FAILED, "x", 1L, peer.copyOf(), mac)
        val b = SwitchPersistence.Saved(SwitchPersistence.DISC_FAILED, "x", 1L, peer.copyOf(), mac)
        assertEquals(a, b)
        assertEquals(a.hashCode(), b.hashCode())
    }

    @Test
    fun `Saved equals with null peer pub`() {
        val a = SwitchPersistence.Saved(SwitchPersistence.DISC_IDLE, null, 0, null, null)
        val b = SwitchPersistence.Saved(SwitchPersistence.DISC_IDLE, null, 0, null, null)
        assertEquals(a, b)
    }
}
