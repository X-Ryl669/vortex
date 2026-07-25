package com.vortex.a3.core.notif

import org.json.JSONObject

/**
 * A phone notification forwarded to the laptop over the Noise-sealed BLE
 * link (frame type [com.vortex.a3.core.ble.FrameType.NOTIFICATION]) for
 * desktop display. Tiny by design — app label + title + text + timestamp,
 * no icon — so it fits a single BLE notify. Fields are length-capped at
 * the capture site before this is built.
 *
 * Mirrors Rust `core::notif_mirror::NotificationMirror`.
 */
data class NotificationMirror(
    /** Friendly app label, or the package name as a fallback. */
    val app: String,
    /** Package name (e.g. "org.telegram.messenger") — a stable id the laptop
     *  uses to look up this app's cached icon. Empty for synthetic messages. */
    val appId: String = "",
    val title: String,
    val text: String,
    val ts: Long,
    /** Stable identity (Android StatusBarNotification.key) for dismiss sync. */
    val key: String = "",
    /** True = this is a DISMISS message (clear the mirrored copy by [key]). */
    val dismiss: Boolean = false,
    /** Phone→laptop: action-button labels, in order. */
    val actions: List<String> = emptyList(),
    /** Phone→laptop: index into [actions] that accepts inline text (a
     *  RemoteInput / reply action), or -1 if none. The laptop pops a text
     *  entry for this action since GNOME has no inline-reply. */
    val replyIndex: Int = -1,
    /** laptop→phone INVOKE: index into [actions] to fire; -1 = not an invoke. */
    val invokeIndex: Int = -1,
    /** laptop→phone: reply text for a RemoteInput action (else empty). */
    val reply: String = "",
    /** laptop→phone INVOKE id — dedups the SAME invoke arriving over both the
     *  BLE NOTIFICATION frame and the LAN AppState backstop. 0 = not an invoke. */
    val seq: Long = 0,
    /** laptop→phone CATCH-UP request: true = not a notification/dismiss/invoke
     *  but a request to RE-SEND any active mirrorable notification whose [key]
     *  isn't in [knownKeys]. Recovers a phone→laptop notify dropped in-air (BLE
     *  Notify is fire-and-forget — a dropped frame is otherwise lost for good).*/
    val resync: Boolean = false,
    /** laptop→phone (with [resync]): keys the laptop already displayed, so we
     *  re-send only the missing ones (no re-popup of what the user already saw).*/
    val knownKeys: List<String> = emptyList(),
) {
    fun toJsonBytes(): ByteArray {
        val o = JSONObject()
        o.put("app", app)
        if (appId.isNotEmpty()) o.put("app_id", appId)
        o.put("title", title)
        o.put("text", text)
        o.put("ts", ts)
        if (key.isNotEmpty()) o.put("key", key)
        if (dismiss) o.put("dismiss", true)
        if (actions.isNotEmpty()) o.put("actions", org.json.JSONArray(actions))
        if (replyIndex >= 0) o.put("reply_index", replyIndex)
        if (invokeIndex >= 0) o.put("invoke_index", invokeIndex)
        if (reply.isNotEmpty()) o.put("reply", reply)
        if (seq > 0) o.put("seq", seq)
        if (resync) o.put("resync", true)
        if (knownKeys.isNotEmpty()) o.put("known_keys", org.json.JSONArray(knownKeys))
        return o.toString().toByteArray(Charsets.UTF_8)
    }

    companion object {
        /** Parse a laptop→phone NOTIFICATION payload, or null if malformed. */
        fun fromJsonBytes(bytes: ByteArray): NotificationMirror? = try {
            val o = JSONObject(String(bytes, Charsets.UTF_8))
            NotificationMirror(
                app = o.optString("app", ""),
                appId = o.optString("app_id", ""),
                title = o.optString("title", ""),
                text = o.optString("text", ""),
                ts = o.optLong("ts", 0L),
                key = o.optString("key", ""),
                dismiss = o.optBoolean("dismiss", false),
                actions = o.optJSONArray("actions")?.let { arr ->
                    (0 until arr.length()).map { arr.optString(it, "") }
                } ?: emptyList(),
                replyIndex = o.optInt("reply_index", -1),
                invokeIndex = o.optInt("invoke_index", -1),
                reply = o.optString("reply", ""),
                seq = o.optLong("seq", 0L),
                resync = o.optBoolean("resync", false),
                knownKeys = o.optJSONArray("known_keys")?.let { arr ->
                    (0 until arr.length()).map { arr.optString(it, "") }
                } ?: emptyList(),
            )
        } catch (_: Throwable) {
            null
        }
    }
}
