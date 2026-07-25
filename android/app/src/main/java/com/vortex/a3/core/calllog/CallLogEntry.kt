package com.vortex.a3.core.calllog

import org.json.JSONArray
import org.json.JSONObject

/**
 * One recent call mirrored to the laptop companion's Recents page. `type`
 * follows `CallLog.Calls.TYPE` (1=incoming, 2=outgoing, 3=missed, 5=rejected,
 * 4=voicemail, 6=blocked). `date` is epoch millis; `duration` is seconds.
 * Mirrors Rust `core::call_log::CallLogEntry`.
 */
data class CallLogEntry(
    val id: String,
    val number: String,
    val name: String,
    val type: Int,
    val date: Long,
    val duration: Long,
)

/** Serialize the recent-calls list to a compact JSON array for chunked BLE
 *  transfer (frame [com.vortex.a3.core.ble.FrameType.CALL_LOG]). */
fun callLogToJsonBytes(entries: List<CallLogEntry>): ByteArray {
    val arr = JSONArray()
    for (e in entries) {
        val o = JSONObject()
        o.put("id", e.id)
        o.put("number", e.number)
        o.put("name", e.name)
        o.put("type", e.type)
        o.put("date", e.date)
        o.put("duration", e.duration)
        arr.put(o)
    }
    return arr.toString().toByteArray(Charsets.UTF_8)
}
