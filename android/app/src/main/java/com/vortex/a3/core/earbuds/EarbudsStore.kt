package com.vortex.a3.core.earbuds

import android.content.Context

/**
 * Persists the user's chosen earbud — address + display name — so the
 * card is shown on the home screen even when the buds are disconnected.
 * Mirrors the Linux-side `earbuds_store.rs` JSON file; SharedPrefs is
 * the natural fit on Android.
 *
 * The pairing protocol layer is untouched: this only records the user's
 * UI choice. EncryptedSharedPreferences is overkill (the BD_ADDR is not
 * a secret — Android Settings displays it) so we use plain SharedPrefs.
 */
data class SavedEarbuds(val address: String, val name: String)

object EarbudsStore {
    private const val PREFS = "vortex_earbuds"
    private const val KEY_ADDR = "saved_addr"
    private const val KEY_NAME = "saved_name"

    fun load(context: Context): SavedEarbuds? {
        val p = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val addr = p.getString(KEY_ADDR, null)?.takeIf { it.isNotBlank() } ?: return null
        val name = p.getString(KEY_NAME, null)?.takeIf { it.isNotBlank() } ?: addr
        return SavedEarbuds(address = addr, name = name)
    }

    fun save(context: Context, earbuds: SavedEarbuds) {
        context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY_ADDR, earbuds.address)
            .putString(KEY_NAME, earbuds.name)
            .apply()
    }

    fun clear(context: Context) {
        context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .remove(KEY_ADDR)
            .remove(KEY_NAME)
            .apply()
    }
}
