package com.vortex.a3.core.contacts

import org.json.JSONArray
import org.json.JSONObject

/**
 * One phone contact mirrored to the laptop companion's Contacts page. Tiny by
 * design — name + numbers only (no photo blob; the laptop draws an initial
 * avatar). Mirrors Rust `core::contacts::Contact`.
 */
data class Contact(
    val id: String,
    val name: String,
    val numbers: List<String>,
)

/** Serialize the full contacts list to a compact JSON array for chunked BLE
 *  transfer (frame [com.vortex.a3.core.ble.FrameType.CONTACTS]). */
fun contactsToJsonBytes(contacts: List<Contact>): ByteArray {
    val arr = JSONArray()
    for (c in contacts) {
        val o = JSONObject()
        o.put("id", c.id)
        o.put("name", c.name)
        val nums = JSONArray()
        for (n in c.numbers) nums.put(n)
        o.put("numbers", nums)
        arr.put(o)
    }
    return arr.toString().toByteArray(Charsets.UTF_8)
}
