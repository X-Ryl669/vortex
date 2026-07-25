package com.vortex.a3.core.clipboard

import java.security.MessageDigest

/**
 * Holds the single most-recent outgoing clipboard/share image in memory,
 * keyed by a content token. The phone signals the laptop "image token=X
 * available" (BLE offer); the laptop then PULLS the PNG by that token over
 * the reliable LAN bulk-sync. In-memory (no files) — the image lives only
 * until it's fetched or replaced by the next.
 */
object ClipboardImageStore {
    @Volatile private var token: String = ""
    @Volatile private var bytes: ByteArray? = null

    /** Stash a PNG; returns its token (content hash). */
    @Synchronized
    fun stash(png: ByteArray): String {
        val t = sha256Hex(png).take(16)
        token = t
        bytes = png
        return t
    }

    /** The PNG for [token], or null if it isn't the current one. */
    @Synchronized
    fun getByToken(token: String): ByteArray? =
        if (token.isNotEmpty() && token == this.token) bytes else null

    private fun sha256Hex(data: ByteArray): String {
        val d = MessageDigest.getInstance("SHA-256").digest(data)
        val sb = StringBuilder(d.size * 2)
        for (b in d) sb.append("%02x".format(b))
        return sb.toString()
    }
}
