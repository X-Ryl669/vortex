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

    /**
     * Pull one file the laptop shared — by its `share:` [path] — straight into
     * Downloads, writing each range as it arrives. Peak memory is a few ranges
     * however large the file is, which is why this path has no size cap.
     *
     * Nothing half-written survives a failure: the entry stays IS_PENDING (so
     * no app lists it) until the last byte is in, and is deleted otherwise.
     * Returns true when the whole file landed.
     */
    suspend fun pull(ctx: Context, path: String, rawName: String): Boolean {
        val name = sanitizeName(rawName)
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
        return try {
            val got = resolver.openFileDescriptor(uri, "w")?.use { pfd ->
                java.io.FileOutputStream(pfd.fileDescriptor).channel.use { ch ->
                    com.vortex.a3.core.fs.FsClient.readAll(path, sink = { at, bytes ->
                        // A positional write may take less than asked; finish it.
                        val buf = java.nio.ByteBuffer.wrap(bytes)
                        var pos = at
                        while (buf.hasRemaining()) pos += ch.write(buf, pos)
                    })
                }
            } ?: throw java.io.IOException("cannot open the Downloads entry")
            values.clear()
            values.put(MediaStore.Downloads.IS_PENDING, 0)
            resolver.update(uri, values, null, null)
            Log.i(TAG, "pulled '$name' ($got bytes) to Downloads")
            true
        } catch (e: Exception) {
            Log.w(TAG, "pull '$name' failed: ${e.message}")
            runCatching { resolver.delete(uri, null, null) }
            false
        }
    }

    /**
     * Pull a folder the laptop zipped for us and unpack it into
     * `Downloads/<folder>/`. The archive goes to app cache first — unpacking
     * needs the whole archive, and holding it in memory is what this replaces —
     * and is deleted afterwards whatever happens.
     *
     * [declaredBytes]/[declaredEntries] are the totals the offer announced,
     * which the user consented to; see [saveZipExtracted].
     */
    suspend fun pullZipExtracted(
        ctx: Context,
        path: String,
        zipName: String,
        declaredBytes: Long?,
        declaredEntries: Long?,
    ): Boolean {
        val tmp = java.io.File(ctx.cacheDir, "incoming-${System.nanoTime()}.zip")
        return try {
            com.vortex.a3.core.fs.FsClient.download(path, tmp)
            java.io.FileInputStream(tmp).use {
                extract(ctx, zipName, it, declaredBytes, declaredEntries)
            }
        } catch (e: Exception) {
            Log.w(TAG, "pull of folder '$zipName' failed: ${e.message}")
            false
        } finally {
            tmp.delete()
        }
    }

    // --- folder auto-extract (laptop zipped a folder for us) --------------------
    /** Caps for an archive whose offer did not declare its totals (a laptop
     *  that predates the pull), so a malicious/corrupt one can't bomb storage. */
    private const val LEGACY_MAX_EXTRACT_BYTES = 512L * 1024 * 1024 // 512 MiB uncompressed
    private const val LEGACY_MAX_EXTRACT_ENTRIES = 10_000L

    /** Sanitize one zip-entry component: reject path-traversal, drop control
     *  chars. Returns null if the component is unsafe/empty (caller skips it). */
    private fun safeComponent(raw: String): String? {
        if (raw == "." || raw == "..") return null // zip-slip / no-op
        val cleaned = raw.filter { it.code >= 0x20 && it.code != 0x7f }.trim()
        return cleaned.ifBlank { null }
    }

    /** Legacy entry point: an archive already assembled in memory. */
    fun saveZipExtracted(ctx: Context, zipName: String, zipBytes: ByteArray): Boolean =
        extract(ctx, zipName, java.io.ByteArrayInputStream(zipBytes), null, null)

    /**
     * Extract a `<folder>.zip` (one WE created from a shared folder) back into
     * `Downloads/<folder>/…` via MediaStore, then the caller discards the zip.
     * Zip-slip-safe: every entry path is decomposed and any `..`/absolute
     * component is rejected, so nothing can ever be written outside the target
     * folder. Each entry is streamed to its file, never held whole in memory.
     * Returns true if at least one file landed.
     *
     * Bounded against a decompression bomb by what the offer declared — the
     * uncompressed total and entry count the user consented to — rather than by
     * a fixed size, which any large enough real folder would trip. An archive
     * that expands past its own declaration is not the one that was agreed to.
     * Old laptops declare nothing, and get the old fixed caps.
     */
    private fun extract(
        ctx: Context,
        zipName: String,
        input: java.io.InputStream,
        declaredBytes: Long?,
        declaredEntries: Long?,
    ): Boolean {
        val maxBytes = declaredBytes ?: LEGACY_MAX_EXTRACT_BYTES
        val maxEntries = declaredEntries ?: LEGACY_MAX_EXTRACT_ENTRIES
        val folder = sanitizeName(zipName.removeSuffix(".zip")).ifBlank { "vortex-folder" }
        val resolver = ctx.contentResolver
        val collection = MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        var wrote = 0
        var totalBytes = 0L
        var entries = 0L
        val buf = ByteArray(64 * 1024)
        try {
            java.util.zip.ZipInputStream(input).use { zin ->
                while (true) {
                    val entry = zin.nextEntry ?: break
                    if (entry.isDirectory) { zin.closeEntry(); continue }
                    if (++entries > maxEntries) {
                        Log.w(TAG, "extract '$folder': more entries than declared; stopping")
                        break
                    }

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

                    val values = ContentValues().apply {
                        put(MediaStore.Downloads.DISPLAY_NAME, leaf)
                        put(MediaStore.Downloads.RELATIVE_PATH, relPath)
                        put(MediaStore.Downloads.IS_PENDING, 1)
                    }
                    val uri = resolver.insert(collection, values)
                    if (uri == null) {
                        Log.w(TAG, "extract: insert null for '$relPath$leaf'")
                        zin.closeEntry()
                        continue
                    }
                    // Stream the entry, counting as we go: the bound has to hold
                    // DURING an entry, or one entry could be the whole bomb.
                    var over = false
                    resolver.openOutputStream(uri)?.use { out ->
                        while (true) {
                            val n = zin.read(buf)
                            if (n < 0) break
                            totalBytes += n
                            if (totalBytes > maxBytes) {
                                over = true
                                break
                            }
                            out.write(buf, 0, n)
                        }
                    }
                    zin.closeEntry()
                    if (over) {
                        Log.w(TAG, "extract '$folder': larger than declared; stopping")
                        runCatching { resolver.delete(uri, null, null) }
                        break
                    }
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
