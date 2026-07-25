package com.vortex.a3.service

import android.util.Log
import kotlinx.coroutines.launch

/**
 * App-icon push + live-activity forwarding — split out of [VortexStack]. Phone
 * app logos are rasterised and sent once per session (chunked ICON frames) so
 * the laptop's mirrored notifications / pills show the real app icon; ride/nav
 * "live activities" are forwarded to the laptop top bar over BLE.
 */

/** Render [pkg]'s launcher icon to a small PNG, then push it to each peer
 *  in [VortexStack.ICON_CHUNK]-sized chunks. Drops the package from
 *  [VortexStack.sentIconPkgs] on any failure so it retries. */
internal suspend fun VortexStack.sendAppIcon(pkg: String) {
    val png = renderAppIconPng(pkg)
    if (png == null) { sentIconPkgs.remove(pkg); return }
    val idBytes = pkg.toByteArray(Charsets.UTF_8)
    if (idBytes.size > 255) { sentIconPkgs.remove(pkg); return }
    val total = ((png.size + ICON_CHUNK - 1) / ICON_CHUNK).coerceAtLeast(1)
    if (total > 0xFFFF) { sentIconPkgs.remove(pkg); return }
    var ok = true
    outer@ for (peer in peerStore.list()) {
        val server = gattServer
        if (server == null) { ok = false; break@outer }
        for (idx in 0 until total) {
            val start = idx * ICON_CHUNK
            val end = minOf(start + ICON_CHUNK, png.size)
            val payload = java.io.ByteArrayOutputStream().apply {
                write(idBytes.size and 0xFF)
                write(idBytes)
                write((total ushr 8) and 0xFF); write(total and 0xFF)
                write((idx ushr 8) and 0xFF); write(idx and 0xFF)
                write(png, start, end - start)
            }.toByteArray()
            if (!server.sendIconChunkEncrypted(peer.peerStaticPub, payload)) { ok = false; break@outer }
            kotlinx.coroutines.delay(12) // pace so the BLE stack doesn't drop notifies
        }
    }
    if (ok) Log.i(VortexStack.TAG, "icon sent for $pkg ($total chunks)") else sentIconPkgs.remove(pkg)
}

/** Rasterise a package's icon to a square PNG (or null if unavailable). */
internal fun VortexStack.renderAppIconPng(pkg: String, sizePx: Int = 64): ByteArray? = try {
    val pm = ctx.packageManager
    val drawable = pm.getApplicationIcon(pkg)
    val bmp = android.graphics.Bitmap.createBitmap(
        sizePx, sizePx, android.graphics.Bitmap.Config.ARGB_8888,
    )
    val canvas = android.graphics.Canvas(bmp)
    drawable.setBounds(0, 0, sizePx, sizePx)
    drawable.draw(canvas)
    val out = java.io.ByteArrayOutputStream()
    bmp.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, out)
    bmp.recycle()
    out.toByteArray()
} catch (e: Exception) {
    Log.w(VortexStack.TAG, "renderAppIconPng($pkg): ${e.message}")
    null
}

/** Forward live activities (ride/navigation/delivery ETA pills) to the
 *  laptop's top bar over BLE. Not buffered: live activities update
 *  frequently and only the latest state matters, so a missed tick during
 *  a BLE blip is corrected by the next update (or the `ended` message). */
internal fun VortexStack.forwardLiveActivities() {
    scope.launch {
        VortexService.liveActivityBus.collect { live ->
            if (!com.vortex.a3.core.notif.NotificationMirrorSetting.isEnabled()) return@collect
            val server = gattServer ?: return@collect
            val json = live.toJsonBytes()
            for (peer in peerStore.list()) {
                server.sendLiveActivityEncrypted(peer.peerStaticPub, json)
            }
            // Push the app's real icon once so the live-activity's own tray
            // on the laptop shows the right logo (Yandex Go, Maps, …).
            val pkg = live.appId
            if (pkg.isNotEmpty() && !live.ended && sentIconPkgs.add(pkg)) {
                scope.launch { sendAppIcon(pkg) }
            }
        }
    }
}
