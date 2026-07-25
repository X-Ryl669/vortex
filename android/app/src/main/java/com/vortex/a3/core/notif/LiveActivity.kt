package com.vortex.a3.core.notif

import org.json.JSONObject

/**
 * A "live activity" — an ongoing, in-place-updating phone notification (ride
 * ETA, navigation, delivery, timer, download) mirrored to the laptop's top
 * bar as a persistent pill (live-activity style). Unlike a one-shot
 * [NotificationMirror], the same [key] is re-sent as the source updates, and
 * an [ended] message clears the pill.
 *
 * Tiny by design — fits a single BLE notify. Mirrors Rust
 * `core::live_activity::LiveActivity`.
 */
data class LiveActivity(
    /** Stable identity (Android StatusBarNotification.key) — the same across
     *  every update so the laptop updates one pill in place. */
    val key: String,
    /** Friendly app label (e.g. "Yandex Go"), or the package as a fallback. */
    val app: String = "",
    /** Package name — the laptop looks up this app's cached icon for the
     *  live-activity's own tray icon. */
    val appId: String = "",
    /** Short headline (contentTitle), e.g. "Heading your way". */
    val title: String = "",
    /** Detail line (contentText), e.g. "Arriving in 1 min". */
    val text: String = "",
    /** Extra line (sub-text), e.g. "2.3 km · 6 min". */
    val sub: String = "",
    /** Progress 0..100, or -1 when there's no determinate progress
     *  (indeterminate spinner / category-only live activity). */
    val progress: Int = -1,
    /** True = the activity finished / was removed → clear its pill. */
    val ended: Boolean = false,
    /** Media pill only: whether the player is currently playing. Non-null
     *  marks this activity as a now-playing pill — the laptop shows
     *  prev/play-pause/next transport buttons and picks the ⏸/▶ icon
     *  from this. Null for every non-media live activity. */
    val playing: Boolean? = null,
) {
    fun toJsonBytes(): ByteArray {
        val o = JSONObject()
        o.put("key", key)
        if (app.isNotEmpty()) o.put("app", app)
        if (appId.isNotEmpty()) o.put("app_id", appId)
        if (title.isNotEmpty()) o.put("title", title)
        if (text.isNotEmpty()) o.put("text", text)
        if (sub.isNotEmpty()) o.put("sub", sub)
        o.put("progress", progress)
        if (ended) o.put("ended", true)
        playing?.let { o.put("playing", it) }
        return o.toString().toByteArray(Charsets.UTF_8)
    }

    companion object {
        fun fromJsonBytes(bytes: ByteArray): LiveActivity? = try {
            val o = JSONObject(String(bytes, Charsets.UTF_8))
            LiveActivity(
                key = o.optString("key", ""),
                app = o.optString("app", ""),
                appId = o.optString("app_id", ""),
                title = o.optString("title", ""),
                text = o.optString("text", ""),
                sub = o.optString("sub", ""),
                progress = o.optInt("progress", -1),
                ended = o.optBoolean("ended", false),
                playing = if (o.has("playing")) o.optBoolean("playing") else null,
            )
        } catch (_: Throwable) {
            null
        }
    }
}
