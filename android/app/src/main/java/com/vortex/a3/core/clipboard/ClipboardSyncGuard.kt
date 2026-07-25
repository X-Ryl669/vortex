package com.vortex.a3.core.clipboard

import java.security.MessageDigest

/**
 * Loop guard for clipboard auto-sync — the Android analog of the laptop's
 * `LAST_SYNC_SIG`. When the laptop sends text and we `setPrimaryClip` it, the
 * OS fires our [ClipboardListener]'s change callback; without this guard the
 * listener would forward that exact text straight back to the laptop, looping.
 *
 * The apply paths ([com.vortex.a3.service.VortexStack] `onClipboardReceived` /
 * `onClipboardTextChunk`) call [markApplied] BEFORE `setPrimaryClip`; the
 * listener calls [wasJustApplied] before forwarding and skips a match.
 *
 * Process-wide singleton; a single slot is enough because the clipboard holds
 * one primary item at a time (the latest applied content).
 */
object ClipboardSyncGuard {
    @Volatile private var lastSig: String = ""

    /** SHA-256 (first 16 hex chars) of UTF-8 text — stable, cheap, collision-safe
     *  enough for a loop guard. */
    fun sig(text: String): String = sig(text.toByteArray(Charsets.UTF_8))

    fun sig(bytes: ByteArray): String {
        val d = MessageDigest.getInstance("SHA-256").digest(bytes)
        val sb = StringBuilder(16)
        for (i in 0 until 8) sb.append("%02x".format(d[i]))
        return sb.toString()
    }

    /** Record what we're about to apply, so the listener recognises and skips it. */
    fun markApplied(sig: String) {
        lastSig = sig
    }

    /** True if [sig] is the content we last applied from the laptop (don't bounce). */
    fun wasJustApplied(sig: String): Boolean = sig.isNotEmpty() && sig == lastSig
}
