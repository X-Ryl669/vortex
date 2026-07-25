package com.vortex.a3.core.sms

import org.json.JSONArray
import org.json.JSONObject

/**
 * One SMS mirrored to the laptop companion's Messages page. `type` follows
 * `Telephony.Sms.TYPE` (1=inbox/received, 2=sent). `date` is epoch millis;
 * `thread` groups a conversation. Body is sensitive — Noise-sealed in transit,
 * never logged. Mirrors Rust `core::sms::SmsMessage`.
 */
data class SmsMessage(
    val id: String,
    val address: String,
    val body: String,
    val type: Int,
    val date: Long,
    val thread: Long,
    /** Telephony.Sms.READ: 0=unread, 1=read (received messages). */
    val read: Int,
)

/** Serialize the recent-SMS list to a compact JSON array for chunked BLE
 *  transfer (frame [com.vortex.a3.core.ble.FrameType.SMS]). */
fun smsToJsonBytes(messages: List<SmsMessage>): ByteArray {
    val arr = JSONArray()
    for (m in messages) {
        val o = JSONObject()
        o.put("id", m.id)
        o.put("address", m.address)
        o.put("body", m.body)
        o.put("type", m.type)
        o.put("date", m.date)
        o.put("thread", m.thread)
        o.put("read", m.read)
        arr.put(o)
    }
    return arr.toString().toByteArray(Charsets.UTF_8)
}
