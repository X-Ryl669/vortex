package com.vortex.a3.core.lan

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.ContentValues
import android.content.Context
import android.os.Build
import android.provider.MediaStore
import android.util.Log

/**
 * Saves files PUSHED from the laptop (reverse-direction share, frame `FILE_PUSH`) into
 * the phone's public Downloads via MediaStore (no storage permission needed on
 * Android 10+). Saving is quiet; the caller pops one aggregate "received"
 * notification per batch via [notifyReceived].
 */
object IncomingFile {
    private const val TAG = "VortexIncomingFile"
    private const val CHANNEL = "vortex_files"

    /** Reduce a peer-supplied name to a safe MediaStore DISPLAY_NAME: basename
     *  only (strip any path), no control chars, no leading dots (hidden /
     *  ".."), length-capped. Defends against a compromised-but-trusted peer
     *  sending `../`, `/abs/path`, or `.nomedia`-style names. */
    private fun sanitizeName(raw: String): String {
        val base = raw.substringAfterLast('/').substringAfterLast('\\')
        val cleaned = base
            .filter { it.code >= 0x20 && it.code != 0x7f }
            .trimStart('.')
            .trim()
            .take(200)
        return cleaned.ifBlank { "vortex-file" }
    }

    /** Write one file to Downloads. Returns true on success. No notification. */
    fun save(ctx: Context, rawName: String, bytes: ByteArray): Boolean {
        val name = sanitizeName(rawName)
        return try {
            val resolver = ctx.contentResolver
            val collection = MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
            val values = ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, name)
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
            val uri = resolver.insert(collection, values)
            if (uri == null) {
                Log.w(TAG, "MediaStore insert returned null for '$name'")
                return false
            }
            resolver.openOutputStream(uri)?.use { it.write(bytes) }
            values.clear()
            values.put(MediaStore.Downloads.IS_PENDING, 0)
            resolver.update(uri, values, null, null)
            Log.i(TAG, "saved '$name' (${bytes.size} bytes) to Downloads")
            true
        } catch (e: Exception) {
            Log.w(TAG, "save failed: ${e.message}")
            false
        }
    }

    // --- folder auto-extract (laptop zipped a folder for us) --------------------
    /** Hard caps so a malicious/corrupt archive can't bomb storage. */
    private const val MAX_EXTRACT_BYTES = 512L * 1024 * 1024 // 512 MiB uncompressed
    private const val MAX_EXTRACT_ENTRIES = 10_000

    /** Sanitize one zip-entry component: reject path-traversal, drop control
     *  chars. Returns null if the component is unsafe/empty (caller skips it). */
    private fun safeComponent(raw: String): String? {
        if (raw == "." || raw == "..") return null // zip-slip / no-op
        val cleaned = raw.filter { it.code >= 0x20 && it.code != 0x7f }.trim()
        return cleaned.ifBlank { null }
    }

    /**
     * Extract a `<folder>.zip` (one WE created from a shared folder) back into
     * `Downloads/<folder>/…` via MediaStore, then the caller discards the zip.
     * Zip-slip-safe: every entry path is decomposed and any `..`/absolute
     * component is rejected, so nothing can ever be written outside the target
     * folder. Returns true if at least one file landed.
     */
    fun saveZipExtracted(ctx: Context, zipName: String, zipBytes: ByteArray): Boolean {
        val folder = sanitizeName(zipName.removeSuffix(".zip")).ifBlank { "vortex-folder" }
        val resolver = ctx.contentResolver
        val collection = MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        var wrote = 0
        var totalBytes = 0L
        var entries = 0
        try {
            java.util.zip.ZipInputStream(java.io.ByteArrayInputStream(zipBytes)).use { zin ->
                while (true) {
                    val entry = zin.nextEntry ?: break
                    if (++entries > MAX_EXTRACT_ENTRIES) {
                        Log.w(TAG, "extract '$folder': too many entries; stopping")
                        break
                    }
                    if (entry.isDirectory) { zin.closeEntry(); continue }

                    // Decompose the entry path; reject traversal, keep safe parts.
                    val parts = entry.name.replace('\\', '/').split('/')
                        .mapNotNull { safeComponent(it) }
                    if (parts.isEmpty()) { zin.closeEntry(); continue }
                    val leaf = parts.last()
                    // Sub-dirs UNDER our folder, e.g. "Download/<folder>/a/b".
                    val subDirs = parts.dropLast(1).joinToString("/")
                    val relPath = buildString {
                        append(android.os.Environment.DIRECTORY_DOWNLOADS).append('/').append(folder)
                        if (subDirs.isNotEmpty()) append('/').append(subDirs)
                        append('/')
                    }

                    // Read the entry, bounded against a decompression bomb.
                    val data = zin.readBytes()
                    zin.closeEntry()
                    totalBytes += data.size
                    if (totalBytes > MAX_EXTRACT_BYTES) {
                        Log.w(TAG, "extract '$folder': over size cap; stopping")
                        break
                    }

                    val values = ContentValues().apply {
                        put(MediaStore.Downloads.DISPLAY_NAME, leaf)
                        put(MediaStore.Downloads.RELATIVE_PATH, relPath)
                        put(MediaStore.Downloads.IS_PENDING, 1)
                    }
                    val uri = resolver.insert(collection, values)
                    if (uri == null) {
                        Log.w(TAG, "extract: insert null for '$relPath$leaf'")
                        continue
                    }
                    resolver.openOutputStream(uri)?.use { it.write(data) }
                    values.clear()
                    values.put(MediaStore.Downloads.IS_PENDING, 0)
                    resolver.update(uri, values, null, null)
                    wrote++
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "extract '$folder' failed: ${e.message}")
        }
        Log.i(TAG, "extracted '$folder': $wrote files ($totalBytes bytes) to Downloads")
        return wrote > 0
    }

    /** One notification for a finished batch: "<label> — saved to Downloads". */
    fun notifyReceived(ctx: Context, label: String, count: Int) {
        try {
            val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                nm.createNotificationChannel(
                    NotificationChannel(CHANNEL, "File transfers", NotificationManager.IMPORTANCE_DEFAULT),
                )
            }
            val title = if (count > 1) "$count files received" else "File received"
            val n = androidx.core.app.NotificationCompat.Builder(ctx, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_sys_download_done)
                .setContentTitle(title)
                .setContentText("$label — saved to Downloads")
                .setAutoCancel(true)
                .build()
            nm.notify(label.hashCode(), n)
        } catch (e: Exception) {
            Log.w(TAG, "notify failed: ${e.message}")
        }
    }
}
