package com.vortex.a3.core.media

import android.content.ContentUris
import android.content.Context
import android.net.Uri
import android.provider.MediaStore
import android.util.Log
import org.json.JSONObject

/**
 * Which gallery rows this phone has sent to the laptop, so their deletion can
 * be passed on.
 *
 * A picture copied to the laptop by itself should not outlive the original —
 * deleting it on the phone and finding it still on the laptop a week later is
 * the copy behaving like a leak rather than a convenience. Nothing else knows
 * enough to notice: the watcher only ever looks at rows ABOVE its watermark,
 * and a deleted row is simply absent.
 *
 * So each capture that goes out is remembered as `token → (collection, _id)`,
 * and every bulk-sync round asks which of those ids MediaStore still has. The
 * ones it doesn't are reported to the laptop by token — the content hash it
 * already filed the copy under.
 *
 * **One direction only.** The laptop deleting its copy does NOT come back the
 * other way: removing a row this app did not create needs a system consent
 * dialog on every file (a `RecoverableSecurityException` on Android 10, a
 * `createTrashRequest` above it), so "tidy the laptop, lose the phone's
 * original" cannot be made to happen quietly — and should not be.
 *
 * Kept in `vortex_captures` as one JSON object. Bounded: the oldest entries go
 * first, which at worst means a very old deletion is not mirrored.
 */
object CaptureLedger {
    private const val PREFS = "vortex_captures"
    private const val KEY = "sent"
    private const val TAG = "CaptureLedger"

    /** Most rows to track. Entries are tiny; this is months of captures. */
    private const val MAX_ENTRIES = 2000

    /**
     * How many rounds a deletion is reported before the entry is dropped.
     *
     * The done frame carries no acknowledgement, so a session that breaks just
     * after it was written would lose the news. Repeating costs one integer in
     * a JSON body and the laptop ignores a token it has already trashed.
     */
    private const val REPORT_TIMES = 3

    private data class Row(val collection: String, val id: Long, var reported: Int)

    private var prefs: android.content.SharedPreferences? = null
    private val rows = LinkedHashMap<String, Row>()

    @Synchronized
    fun init(context: Context) {
        if (prefs != null) return
        val p = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs = p
        rows.clear()
        try {
            val obj = JSONObject(p.getString(KEY, "{}") ?: "{}")
            for (token in obj.keys()) {
                val o = obj.optJSONObject(token) ?: continue
                rows[token] = Row(
                    collection = o.optString("c", "images"),
                    id = o.optLong("i", -1L),
                    reported = o.optInt("r", 0),
                )
            }
        } catch (e: Exception) {
            Log.w(TAG, "ledger unreadable (${e.message}); starting empty")
        }
    }

    /** Remember that [token]'s bytes came from [uri] in [collection]. */
    @Synchronized
    fun record(token: String, collection: String, uri: Uri) {
        if (token.isEmpty()) return
        val id = ContentUris.parseId(uri)
        rows.remove(token) // re-insert to refresh recency
        rows[token] = Row(collection, id, 0)
        while (rows.size > MAX_ENTRIES) {
            val oldest = rows.keys.iterator().next()
            rows.remove(oldest)
        }
        persist()
    }

    /**
     * Tokens whose rows MediaStore no longer has, and which still have reports
     * left. Called once per bulk-sync round; marks what it returns as reported
     * and drops entries that have been reported enough.
     */
    @Synchronized
    fun deletedTokens(context: Context): List<String> {
        if (rows.isEmpty()) return emptyList()
        val alive = HashSet<Long>()
        for (collection in rows.values.map { it.collection }.toSet()) {
            alive += aliveIds(context, collection, rows.filterValues { it.collection == collection })
        }
        val gone = ArrayList<String>()
        val drop = ArrayList<String>()
        for ((token, row) in rows) {
            if (row.id in alive || row.id < 0) continue
            gone += token
            row.reported++
            if (row.reported >= REPORT_TIMES) drop += token
        }
        if (gone.isNotEmpty()) {
            for (t in drop) rows.remove(t)
            persist()
            Log.i(TAG, "${gone.size} capture(s) deleted on this phone; telling the laptop")
        }
        return gone
    }

    /** The subset of [tracked]'s ids that [collection] still holds. */
    private fun aliveIds(
        context: Context,
        collection: String,
        tracked: Map<String, Row>,
    ): Set<Long> {
        val ids = tracked.values.map { it.id }.filter { it >= 0 }
        if (ids.isEmpty()) return emptySet()
        val uri = collectionUri(collection)
        // One query for the whole set. A per-row existence check would be one
        // provider round-trip per capture, every twelve seconds, for ever.
        val placeholders = ids.joinToString(",") { "?" }
        return try {
            context.contentResolver.query(
                uri,
                arrayOf(MediaStore.MediaColumns._ID),
                "${MediaStore.MediaColumns._ID} IN ($placeholders)",
                ids.map { it.toString() }.toTypedArray(),
                null,
            )?.use { c ->
                val out = HashSet<Long>()
                while (c.moveToNext()) out += c.getLong(0)
                out
            } ?: emptySet()
        } catch (e: SecurityException) {
            // No grant → we cannot tell absent from unreadable, and calling
            // every tracked row deleted would wipe the laptop's copies.
            Log.i(TAG, "no media grant; deletions not checked")
            ids.toSet()
        } catch (e: Exception) {
            Log.w(TAG, "deletion check failed (${e.message}); assuming everything is alive")
            ids.toSet()
        }
    }

    private fun collectionUri(collection: String): Uri = when (collection) {
        "video" -> MediaStore.Video.Media.EXTERNAL_CONTENT_URI
        else -> MediaStore.Images.Media.EXTERNAL_CONTENT_URI
    }

    private fun persist() {
        val p = prefs ?: return
        val obj = JSONObject()
        for ((token, row) in rows) {
            obj.put(
                token,
                JSONObject().put("c", row.collection).put("i", row.id).put("r", row.reported),
            )
        }
        p.edit().putString(KEY, obj.toString()).apply()
    }
}
