package com.vortex.a3.core.handoff

import org.json.JSONObject

/**
 * A page the phone hands off to the laptop (continuity style). Mirrors the
 * Rust `core::handoff::HandoffEvent`; rides the Noise-sealed AUDIO_SIGNAL stream
 * as frame [com.vortex.a3.core.ble.FrameType.HANDOFF].
 *
 * @param url      the page URL (empty = "stop handing off" → clears the laptop pill)
 * @param title    page/tab title for the pill label
 * @param appId    source app package (e.g. com.android.chrome) for its icon
 * @param openNow  true = an explicit Share → the laptop opens it immediately;
 *                 false = the live accessibility read → a "continue" pill
 */
data class HandoffEvent(
    val url: String,
    val title: String = "",
    val appId: String = "",
    val openNow: Boolean = false,
) {
    fun toJsonBytes(): ByteArray {
        val o = JSONObject()
        o.put("url", url)
        if (title.isNotEmpty()) o.put("title", title)
        if (appId.isNotEmpty()) o.put("app_id", appId)
        o.put("open_now", openNow)
        return o.toString().toByteArray(Charsets.UTF_8)
    }
}
