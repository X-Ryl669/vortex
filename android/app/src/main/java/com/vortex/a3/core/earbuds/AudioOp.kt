package com.vortex.a3.core.earbuds

import org.json.JSONException
import org.json.JSONObject

/**
 * Earbuds-switch op wire types (the earbuds-switch design notes §4).
 *
 * Mirrors `l3/daemon/src/core/audio_op.rs`. These ride frame type
 * `AUDIO_OP = 0x32` inside the post-handshake Noise AEAD channel.
 *
 * Per-peer monotonic [nonce] rejects replay — the receiver keeps the
 * highest accepted nonce per peer and drops anything `<= in_nonce`.
 */
sealed class AudioOp {
    object Request : AudioOp()
    /** Sender holds the buds and is asking the recipient to claim them.
     *  Recipient runs its own initiator flow on receipt — this is how
     *  bidirectional swap works without needing the recipient to be
     *  reachable as a server. */
    object Claim : AudioOp()
    object Approve : AudioOp()
    data class Reject(val reason: RejectReason) : AudioOp()
    object Released : AudioOp()
    object Done : AudioOp()
    data class Failed(val stage: Stage, val message: String) : AudioOp()
}

enum class RejectReason(val wire: String) {
    Busy("busy"),
    InCall("in_call"),
    RecentSwitch("recent_switch"),
    Unknown("unknown");

    companion object {
        fun fromWire(s: String?): RejectReason? = entries.firstOrNull { it.wire == s }
    }
}

enum class Stage(val wire: String) {
    Disconnect("disconnect"),
    Connect("connect"),
    WaitReady("wait_ready");

    companion object {
        fun fromWire(s: String?): Stage? = entries.firstOrNull { it.wire == s }
    }
}

/**
 * Wire envelope. Serialized as JSON, AEAD-wrapped inside an
 * `AUDIO_OP` frame. JSON is interoperable with the Rust serde-tagged
 * representation:
 *   `{"nonce":N,"op":{"kind":"reject","reason":"in_call"},"mac":"...","ts":T}`
 */
data class AudioOpFrame(
    val nonce: Long,
    val op: AudioOp,
    val mac: String,
    val ts: Long,
) {
    fun toJsonBytes(): ByteArray {
        val obj = JSONObject()
        obj.put("nonce", nonce)
        obj.put("op", encodeOp(op))
        obj.put("mac", mac)
        obj.put("ts", ts)
        return obj.toString().toByteArray(Charsets.UTF_8)
    }

    companion object {
        fun fromJsonBytes(bytes: ByteArray): AudioOpFrame? {
            return try {
                val obj = JSONObject(String(bytes, Charsets.UTF_8))
                val nonce = obj.optLong("nonce", -1L)
                if (nonce < 0) return null
                val opObj = obj.optJSONObject("op") ?: return null
                val op = decodeOp(opObj) ?: return null
                val mac = obj.optString("mac", "")
                val ts = obj.optLong("ts", 0L)
                if (mac.isBlank()) return null
                AudioOpFrame(nonce, op, mac, ts)
            } catch (_: JSONException) {
                null
            }
        }

        private fun encodeOp(op: AudioOp): JSONObject {
            val o = JSONObject()
            when (op) {
                AudioOp.Request -> o.put("kind", "request")
                AudioOp.Claim -> o.put("kind", "claim")
                AudioOp.Approve -> o.put("kind", "approve")
                is AudioOp.Reject -> {
                    o.put("kind", "reject")
                    o.put("reason", op.reason.wire)
                }
                AudioOp.Released -> o.put("kind", "released")
                AudioOp.Done -> o.put("kind", "done")
                is AudioOp.Failed -> {
                    o.put("kind", "failed")
                    o.put("stage", op.stage.wire)
                    o.put("message", op.message)
                }
            }
            return o
        }

        private fun decodeOp(o: JSONObject): AudioOp? {
            return when (o.optString("kind")) {
                "request" -> AudioOp.Request
                "claim" -> AudioOp.Claim
                "approve" -> AudioOp.Approve
                "reject" -> {
                    val r = RejectReason.fromWire(o.optString("reason")) ?: return null
                    AudioOp.Reject(r)
                }
                "released" -> AudioOp.Released
                "done" -> AudioOp.Done
                "failed" -> {
                    val s = Stage.fromWire(o.optString("stage")) ?: return null
                    val m = o.optString("message", "")
                    AudioOp.Failed(s, m)
                }
                else -> null
            }
        }
    }
}
