package com.vortex.a3.core.clipboard

import java.security.MessageDigest

/**
 * Holds recent outgoing FILE blobs (instant-share style) keyed by a content
 * token, so MULTIPLE files selected in one share all survive until the laptop
 * pulls each over LAN. Unlike [ClipboardImageStore] (single most-recent image),
 * this keeps several; the oldest is evicted past [MAX_ENTRIES].
 */
object ClipboardBlobStore {
    private const val MAX_ENTRIES = 32

    // Insertion-ordered so eviction drops the oldest first.
    private val blobs = LinkedHashMap<String, ByteArray>()

    /** Stash [bytes]; returns the content token the laptop pulls it by. */
    @Synchronized
    fun stash(bytes: ByteArray): String {
        val token = sha256Hex(bytes).take(16)
        blobs.remove(token) // re-insert to refresh recency
        blobs[token] = bytes
        while (blobs.size > MAX_ENTRIES) {
            val oldest = blobs.keys.iterator().next()
            blobs.remove(oldest)
        }
        return token
    }

    /** The bytes for [token], or null if unknown/evicted. */
    @Synchronized
    fun getByToken(token: String): ByteArray? =
        if (token.isNotEmpty()) blobs[token] else null

    private fun sha256Hex(data: ByteArray): String {
        val d = MessageDigest.getInstance("SHA-256").digest(data)
        val sb = StringBuilder(d.size * 2)
        for (b in d) sb.append("%02x".format(b))
        return sb.toString()
    }
}
