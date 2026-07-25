package com.vortex.a3.core.appstate

import android.content.Context

/**
 * Persisted monotonic sequence for [AppState.lockCommandSeq]. SharedPrefs
 * (not in-memory) so an app restart can never reissue an old value — the
 * laptop drops any seq it has already executed, and a reused number would
 * make a fresh user tap look like a replay and get silently ignored.
 */
object LockCommandSeq {
    private const val PREFS = "vortex_lock"
    private const val KEY_SEQ = "seq"

    /** Allocate the next sequence number (starts at 1). */
    fun next(context: Context): Long {
        val p = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val n = p.getLong(KEY_SEQ, 0L) + 1L
        p.edit().putLong(KEY_SEQ, n).apply()
        return n
    }
}
