package com.vortex.a3.core.clipboard

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log

/** A file the phone is sending to the laptop (bytes + display name + MIME). */
data class ClipboardOutgoingFile(val bytes: ByteArray, val name: String, val mime: String)

/**
 * Reads an arbitrary clipboard / shared `content://` URI into a
 * [ClipboardOutgoingFile] for phone→laptop FILE sync. Used by both the Quick
 * Settings quick-send and the share-sheet target. Returns null if it isn't
 * readable or exceeds the LAN size cap.
 */
object ClipboardFileReader {
    /** Mirrors the Rust `clipboard_mirror::MAX_FILE_BYTES`. */
    const val MAX_FILE_BYTES = 64L * 1024 * 1024

    private const val TAG = "ClipboardFileOut"

    fun read(context: Context, uri: Uri): ClipboardOutgoingFile? {
        return try {
            readInner(context, uri)
        } catch (e: Exception) {
            Log.w(TAG, "file read failed: ${e.message}")
            null
        }
    }

    private fun readInner(context: Context, uri: Uri): ClipboardOutgoingFile? {
        val cr = context.contentResolver
        val mime = cr.getType(uri) ?: "application/octet-stream"
        val name = displayName(context, uri) ?: "file"
        // Ask how big it is BEFORE reading it. `readBytes()` pulls the whole
        // file into the service's heap, so checking the cap afterwards means
        // the one thing the cap exists to prevent — a file far too large to
        // hold — has already happened. Harmless while this only ever saw
        // screenshots; a phone video is hundreds of megabytes.
        val declared = declaredSize(context, uri)
        if (declared != null && declared > MAX_FILE_BYTES) {
            Log.i(TAG, "file too large ($declared bytes) — not sent")
            return null
        }
        val bytes = cr.openInputStream(uri)?.use { it.readBytes() }
        return when {
            bytes == null -> null
            bytes.isEmpty() -> null
            // Backstop: SIZE is provider-supplied and may be absent or wrong.
            bytes.size > MAX_FILE_BYTES -> {
                Log.i(TAG, "file too large (${bytes.size} bytes) — not sent")
                null
            }
            else -> ClipboardOutgoingFile(bytes, name, mime)
        }
    }

    /** The provider's own SIZE for [uri], or null when it doesn't report one. */
    private fun declaredSize(context: Context, uri: Uri): Long? = try {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.SIZE), null, null, null)
            ?.use { c ->
                val idx = c.getColumnIndex(OpenableColumns.SIZE)
                if (c.moveToFirst() && idx >= 0 && !c.isNull(idx)) c.getLong(idx) else null
            }
    } catch (_: Exception) {
        null
    }

    private fun displayName(context: Context, uri: Uri): String? = try {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { c ->
                if (c.moveToFirst()) {
                    val idx = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    if (idx >= 0) c.getString(idx) else null
                } else {
                    null
                }
            }
            ?: uri.lastPathSegment
    } catch (_: Exception) {
        uri.lastPathSegment
    }
}
