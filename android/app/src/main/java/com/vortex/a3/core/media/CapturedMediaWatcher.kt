package com.vortex.a3.core.media

import android.content.ContentUris
import android.content.Context
import android.database.ContentObserver
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.MediaStore
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** What kind of picture the gallery just gained — decides the toggle that
 *  gates it and the folder it lands in on the laptop. */
enum class CapturedKind(
    /** The value carried in the file OFFER's `kind` field; the laptop maps it
     *  to a subfolder itself, so the phone never names a path. */
    val wire: String,
) {
    SCREENSHOT("screenshot"),
    PHOTO("photo"),
}

/** One finished picture in the gallery that a switched-on toggle covers. */
data class CapturedMedia(
    val uri: Uri,
    val id: Long,
    val name: String,
    val mime: String,
    val bytes: Long,
    val kind: CapturedKind,
)

/**
 * Watches the gallery for a NEW screenshot or camera photo and reports each
 * one once, so the laptop can be offered it — the trigger half of
 * "screenshots and photos land on the laptop by themselves". The delivery
 * half is the share-sheet pipeline, untouched: this only replaces the tap on
 * "Share → Vortex".
 *
 * Same shape as [com.vortex.a3.core.calllog.CallLogProvider]: a
 * `ContentObserver` on `MediaStore.Images`, a quiet window to collapse the
 * burst of change notices one picture produces, then a query. What differs
 * is that a picture is not a snapshot to re-send whole — it is a row to
 * report exactly once, which is what the watermark and the seen-set below
 * are for:
 *
 *  - **`_id` watermark.** The observer says only "something changed", so each
 *    scan asks for rows with `_id` above the highest one already handled.
 *    `_id` is the media table's autoincrement key, so it climbs and never
 *    reuses. Seeded with the CURRENT maximum at start: the pictures already on
 *    the phone are not a backlog to ship, and a switch turned on tonight must
 *    not empty the camera roll onto the laptop.
 *  - **Pending rows.** On Android 10 the screenshot service inserts the row
 *    with `IS_PENDING=1` and only clears it once the bytes are written — the
 *    same flag [com.vortex.a3.core.lan.IncomingFile] sets on the files IT
 *    writes. Reading such a row yields a truncated file, so a pending row
 *    holds the watermark and the scan comes back for it. A row that stays
 *    pending past [PENDING_GIVE_UP_MS] is a capture that was abandoned
 *    (the editor cancelled, the app died); it is stepped over so it can't
 *    block everything after it for ever.
 *  - **Seen-set.** Rows are handled in `_id` order, but a finished row can sit
 *    ABOVE a pending one, in which case it goes out while the watermark stays
 *    below it — and would be found again next scan. The set says which ids
 *    already went. It also absorbs MIUI's gallery, which rewrites a file after
 *    insert (EXIF, thumbnail) and fires more change notices for the same id.
 *
 * Which pictures count is a bucket test, `Screenshots` or `Camera` — the
 * folder names every Android ROM uses for its own captures — and a freshness
 * test on `DATE_ADDED`: a scan runs within seconds of the change it answers,
 * so a row added more than [FRESHNESS_MARGIN_SEC] ago is not something the
 * user just took. That is what keeps a re-index or a restore (OLD pictures
 * under NEW ids) off the laptop, and what bounds a late permission grant to
 * the capture the user made to try it, not the album behind it.
 *
 * Needs READ_EXTERNAL_STORAGE (READ_MEDIA_IMAGES from Android 13), granted at
 * runtime and optional: without it the query is silently empty on Android 10,
 * so the grant is checked explicitly and its absence logged once, and the
 * rest of the app is unaffected.
 *
 * Lives and dies with the service the observer is registered from. MIUI kills
 * that service freely; while it is down nothing is watched, and pictures taken
 * then are NOT sent later (the watermark re-seeds on start). That is the
 * intended shape — "lands seconds later" is a live feature, not a sync — and
 * it means this class never depends on the observer having survived.
 */
class CapturedMediaWatcher(
    private val context: Context,
    private val onCaptured: (CapturedMedia) -> Unit,
) {
    private val tag = "CapturedMedia"
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var observer: ContentObserver? = null
    private var pendingScan: Job? = null

    /** The watermark seed. Every scan joins it first: a change notice that
     *  beat the seed would otherwise be answered against a watermark of -1,
     *  and the freshness test is the only thing left between that and the
     *  last two minutes of the gallery going out. */
    private var seedJob: Job? = null

    /** Highest `_id` every row up to which has been handled (sent, skipped,
     *  or given up on). */
    @Volatile private var watermark: Long = -1L

    /** Ids handled ABOVE the watermark (a finished row past a pending one),
     *  and ids MIUI keeps re-announcing. Bounded; the watermark eventually
     *  passes everything in it. */
    private val seen = LinkedHashSet<Long>()

    /** When each still-pending row was first noticed, for [PENDING_GIVE_UP_MS]. */
    private val pendingSince = HashMap<Long, Long>()

    /** Logged-once guard for the missing permission. */
    @Volatile private var permissionWarned = false

    companion object {
        /** Quiet window after a change notice before the scan. One screenshot
         *  produces several notices (insert, the pending clear, MIUI's
         *  rewrite); this folds them into one query. Short, because "seconds
         *  later" is the promise. */
        private const val SCAN_DEBOUNCE_MS = 1_000L

        /** Re-scan interval while a row is still pending. */
        private const val PENDING_RECHECK_MS = 2_000L

        /** How long a row may stay pending before it is stepped over. */
        private const val PENDING_GIVE_UP_MS = 60_000L

        /** How old (by `DATE_ADDED`, wall-clock seconds) a row may be when a
         *  scan reaches it and still count as "just taken". Wide enough for a
         *  pending row that took its full [PENDING_GIVE_UP_MS] to finish. */
        private const val FRESHNESS_MARGIN_SEC = 120L

        /** Bound on [seen]. Far above any plausible burst. */
        private const val SEEN_MAX = 512

        /** Rows per scan. Bounds a burst-mode session; anything past this is
         *  picked up by the next notice. */
        private const val SCAN_LIMIT = 50

        private val COLLECTION: Uri = MediaStore.Images.Media.EXTERNAL_CONTENT_URI

        private val PROJECTION = arrayOf(
            MediaStore.Images.Media._ID,
            MediaStore.Images.Media.DISPLAY_NAME,
            MediaStore.Images.Media.BUCKET_DISPLAY_NAME,
            MediaStore.Images.Media.RELATIVE_PATH,
            MediaStore.Images.Media.MIME_TYPE,
            MediaStore.Images.Media.SIZE,
            MediaStore.Images.Media.IS_PENDING,
            MediaStore.Images.Media.DATE_ADDED,
        )
    }

    fun start() {
        if (observer != null) return
        MediaAutoShareSetting.init(context)
        // Seed the watermark. Done on IO — it is a query — and joined by every
        // scan (see [seedJob]), so registering the observer right away is safe.
        seedJob = scope.launch {
            watermark = currentMaxId()
            Log.i(tag, "watching the gallery from _id=$watermark")
        }
        val obs = object : ContentObserver(Handler(Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean, uri: Uri?) {
                scheduleScan(SCAN_DEBOUNCE_MS)
            }
        }
        try {
            // Descendants too: the notice for a single image arrives on
            // `…/images/media/<id>`, not on the collection itself.
            context.contentResolver.registerContentObserver(COLLECTION, true, obs)
            observer = obs
        } catch (e: Exception) {
            Log.w(tag, "registerContentObserver: ${e.message}")
        }
    }

    fun stop() {
        observer?.let {
            try { context.contentResolver.unregisterContentObserver(it) } catch (_: Exception) {}
        }
        observer = null
        pendingScan?.cancel()
        scope.coroutineContext[Job]?.cancel()
    }

    private fun scheduleScan(afterMs: Long) {
        // Nothing switched on → no query at all. The observer stays registered
        // (it costs nothing) so a toggle flipped later needs no restart.
        if (!MediaAutoShareSetting.anyEnabled()) return
        pendingScan?.cancel()
        pendingScan = scope.launch {
            delay(afterMs)
            seedJob?.join()
            scan()
        }
    }

    private fun hasPermission(): Boolean =
        androidx.core.content.ContextCompat.checkSelfPermission(
            context,
            mediaReadPermission(),
        ) == android.content.pm.PackageManager.PERMISSION_GRANTED

    /** MAX(_id) right now, or -1 on an empty/unreadable gallery. Without the
     *  storage grant Android 10 answers with only OUR rows, which is fine for
     *  a seed: the watermark is re-seeded on the first permitted scan (see
     *  [scan]) so a grant given later doesn't unleash the gallery. */
    private fun currentMaxId(): Long = try {
        context.contentResolver.query(
            COLLECTION,
            arrayOf(MediaStore.Images.Media._ID),
            null,
            null,
            "${MediaStore.Images.Media._ID} DESC LIMIT 1",
        )?.use { c -> if (c.moveToFirst()) c.getLong(0) else -1L } ?: -1L
    } catch (e: Exception) {
        Log.w(tag, "seed query: ${e.message}")
        -1L
    }

    /** True until the first scan that ran WITH the permission, so the seed can
     *  be redone against the real gallery. */
    @Volatile private var seededWithPermission = false

    private fun scan() {
        if (!hasPermission()) {
            if (!permissionWarned) {
                permissionWarned = true
                Log.i(tag, "storage permission not granted; screenshots/photos stay on the phone")
            }
            return
        }
        if (!seededWithPermission) {
            // The seed may have run unprivileged (see currentMaxId), in which
            // case it sits far below the real gallery. Everything present now
            // is history, not a capture — except the picture the user just
            // took to see the feature work, which is what scheduled this scan.
            // Re-seed to just below the oldest FRESH row, so that one goes and
            // the album behind it does not.
            seededWithPermission = true
            permissionWarned = false
            watermark = maxOf(watermark, freshWatermark())
        }
        var stillPending = false
        try {
            context.contentResolver.query(
                COLLECTION,
                PROJECTION,
                "${MediaStore.Images.Media._ID} > ?",
                arrayOf(watermark.toString()),
                "${MediaStore.Images.Media._ID} ASC LIMIT $SCAN_LIMIT",
            )?.use { c ->
                val idIdx = c.getColumnIndexOrThrow(MediaStore.Images.Media._ID)
                val nameIdx = c.getColumnIndex(MediaStore.Images.Media.DISPLAY_NAME)
                val bucketIdx = c.getColumnIndex(MediaStore.Images.Media.BUCKET_DISPLAY_NAME)
                val relIdx = c.getColumnIndex(MediaStore.Images.Media.RELATIVE_PATH)
                val mimeIdx = c.getColumnIndex(MediaStore.Images.Media.MIME_TYPE)
                val sizeIdx = c.getColumnIndex(MediaStore.Images.Media.SIZE)
                val pendIdx = c.getColumnIndex(MediaStore.Images.Media.IS_PENDING)
                val addedIdx = c.getColumnIndex(MediaStore.Images.Media.DATE_ADDED)
                // The watermark only advances over a CONTIGUOUS run of handled
                // rows from the bottom; the first pending row stops it.
                var advanceTo = watermark
                var blocked = false
                while (c.moveToNext()) {
                    val id = c.getLong(idIdx)
                    val pending = pendIdx >= 0 && c.getInt(pendIdx) == 1
                    if (pending) {
                        val now = android.os.SystemClock.elapsedRealtime()
                        val since = pendingSince.getOrPut(id) { now }
                        if (now - since < PENDING_GIVE_UP_MS) {
                            stillPending = true
                            blocked = true
                            continue
                        }
                        Log.w(tag, "_id=$id pending for over a minute; stepping over it")
                        pendingSince.remove(id)
                        remember(id)
                        if (!blocked) advanceTo = id
                        continue
                    }
                    pendingSince.remove(id)
                    if (!blocked) advanceTo = id
                    if (id in seen) continue
                    remember(id)

                    val kind = classifyCapture(
                        bucket = if (bucketIdx >= 0) c.getString(bucketIdx) else null,
                        relPath = if (relIdx >= 0) c.getString(relIdx) else null,
                    ) ?: continue
                    val on = when (kind) {
                        CapturedKind.SCREENSHOT -> MediaAutoShareSetting.screenshotsEnabled()
                        CapturedKind.PHOTO -> MediaAutoShareSetting.photosEnabled()
                    }
                    if (!on) continue
                    val added = if (addedIdx >= 0) c.getLong(addedIdx) else 0L
                    if (added < nowSec() - FRESHNESS_MARGIN_SEC) {
                        Log.i(tag, "_id=$id is an old picture under a new id (re-index?); not sent")
                        continue
                    }
                    val name = (if (nameIdx >= 0) c.getString(nameIdx) else null)
                        ?.takeIf { it.isNotBlank() } ?: "image-$id"
                    val mime = (if (mimeIdx >= 0) c.getString(mimeIdx) else null)
                        ?.takeIf { it.isNotBlank() } ?: "image/*"
                    val size = if (sizeIdx >= 0) c.getLong(sizeIdx) else 0L
                    val media = CapturedMedia(
                        uri = ContentUris.withAppendedId(COLLECTION, id),
                        id = id,
                        name = name,
                        mime = mime,
                        bytes = size,
                        kind = kind,
                    )
                    Log.i(tag, "new ${kind.name.lowercase()} _id=$id ($size bytes)")
                    try {
                        onCaptured(media)
                    } catch (e: Exception) {
                        Log.w(tag, "onCaptured threw: ${e.message}")
                    }
                }
                if (advanceTo > watermark) watermark = advanceTo
            }
        } catch (e: SecurityException) {
            // The grant was revoked mid-run (Settings → Permissions). Same
            // outcome as never granted; say so once.
            if (!permissionWarned) {
                permissionWarned = true
                Log.i(tag, "storage permission revoked; screenshots/photos stay on the phone")
            }
        } catch (e: Exception) {
            Log.w(tag, "scan: ${e.message}")
        }
        if (stillPending) scheduleScan(PENDING_RECHECK_MS)
    }

    private fun nowSec(): Long = System.currentTimeMillis() / 1000L

    /** The `_id` just below the oldest row that still counts as fresh — so a
     *  first permitted scan handles the capture that triggered it and nothing
     *  older. Falls back to MAX(_id) when nothing is fresh. */
    private fun freshWatermark(): Long = try {
        val cutoff = nowSec() - FRESHNESS_MARGIN_SEC
        context.contentResolver.query(
            COLLECTION,
            arrayOf(MediaStore.Images.Media._ID),
            "${MediaStore.Images.Media.DATE_ADDED} >= ?",
            arrayOf(cutoff.toString()),
            "${MediaStore.Images.Media._ID} ASC LIMIT 1",
        )?.use { c -> if (c.moveToFirst()) c.getLong(0) - 1 else currentMaxId() } ?: currentMaxId()
    } catch (e: Exception) {
        currentMaxId()
    }

    private fun remember(id: Long) {
        seen += id
        while (seen.size > SEEN_MAX) {
            val oldest = seen.iterator().next()
            seen.remove(oldest)
        }
    }

}

/** Screenshot or camera photo, by the folder the ROM put it in; anything
 *  else (a download, a messenger's saved image, an edit) is not ours. The
 *  bucket is the folder's own name; RELATIVE_PATH's last segment is checked
 *  too, because a ROM may localise the bucket label while the path on disk
 *  stays `DCIM/Screenshots/`. Pure, so it is unit-tested without a provider. */
internal fun classifyCapture(bucket: String?, relPath: String?): CapturedKind? {
    val b = bucket?.trim()?.lowercase().orEmpty()
    val p = relPath?.trim()?.trimEnd('/')?.substringAfterLast('/')?.lowercase().orEmpty()
    return when {
        b == "screenshots" || p == "screenshots" -> CapturedKind.SCREENSHOT
        b == "camera" || p == "camera" -> CapturedKind.PHOTO
        else -> null
    }
}

/** The runtime permission that lets us read other apps' pictures: the
 *  granular one from Android 13, the legacy storage one below it (where the
 *  granular one does not exist). One place, so the checks and the request
 *  can't disagree. */
fun mediaReadPermission(): String =
    if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
        android.Manifest.permission.READ_MEDIA_IMAGES
    } else {
        @Suppress("DEPRECATION")
        android.Manifest.permission.READ_EXTERNAL_STORAGE
    }
