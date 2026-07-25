package com.vortex.a3.core.clipboard

/**
 * Chunked clipboard-image transfer (CLIPBOARD_IMAGE 0x41), `[total][idx][data]`
 * wire — matches the Rust `clipboard_mirror` side.
 */
object ClipboardImage {
    /** PNG bytes per chunk — kept under the BLE notify MTU (matches the Rust
     *  IMAGE_CHUNK_BYTES). */
    const val CHUNK_BYTES = 454

    /** Phone→laptop image cap; larger images aren't sent over BLE. */
    const val MAX_BLE_IMAGE_BYTES = 1_048_576

    /** Split a PNG into `[total u16][idx u16][chunk]` chunk payloads. */
    fun buildChunks(png: ByteArray): List<ByteArray> {
        val total = ((png.size + CHUNK_BYTES - 1) / CHUNK_BYTES).coerceAtLeast(1)
        val out = ArrayList<ByteArray>(total)
        for (idx in 0 until total) {
            val start = idx * CHUNK_BYTES
            val end = minOf(start + CHUNK_BYTES, png.size)
            val payload = ByteArray(4 + (end - start))
            payload[0] = ((total ushr 8) and 0xFF).toByte()
            payload[1] = (total and 0xFF).toByte()
            payload[2] = ((idx ushr 8) and 0xFF).toByte()
            payload[3] = (idx and 0xFF).toByte()
            System.arraycopy(png, start, payload, 4, end - start)
            out.add(payload)
        }
        return out
    }
}

/**
 * Chunked LONG clipboard-text transfer (CLIPBOARD_TEXT 0x43), `[total][idx][utf8]`
 * wire — matches the Rust `clipboard_mirror::build_text_chunks`. Short text rides
 * a single CLIPBOARD (0x40) frame; only text over [MAX_SINGLE_FRAME_TEXT_BYTES]
 * (UTF-8) takes this path. Reassembled by [ClipboardImageAssembler] (byte-blind),
 * then UTF-8-decoded.
 */
object ClipboardText {
    const val CHUNK_BYTES = 454

    /** Text whose UTF-8 form is this many bytes or fewer rides one CLIPBOARD
     *  frame; longer text is chunked. Matches the Rust side. */
    const val MAX_SINGLE_FRAME_TEXT_BYTES = 400

    /** Hard cap on synced text (chars). Matches Rust MAX_CLIPBOARD_TEXT_CHARS. */
    const val MAX_TEXT_CHARS = 65_536

    /** Split UTF-8 text into `[total u16][idx u16][utf8-chunk]` payloads, never
     *  splitting a code point across a chunk boundary. */
    fun buildChunks(text: String): List<ByteArray> {
        val groups = ArrayList<ByteArray>()
        val buf = java.io.ByteArrayOutputStream()
        var i = 0
        while (i < text.length) {
            val cp = text.codePointAt(i)
            val b = String(Character.toChars(cp)).toByteArray(Charsets.UTF_8)
            if (buf.size() > 0 && buf.size() + b.size > CHUNK_BYTES) {
                groups.add(buf.toByteArray()); buf.reset()
            }
            buf.write(b)
            i += Character.charCount(cp)
        }
        if (buf.size() > 0 || groups.isEmpty()) groups.add(buf.toByteArray())
        val total = groups.size.coerceAtLeast(1)
        val out = ArrayList<ByteArray>(total)
        for (idx in groups.indices) {
            val g = groups[idx]
            val payload = ByteArray(4 + g.size)
            payload[0] = ((total ushr 8) and 0xFF).toByte()
            payload[1] = (total and 0xFF).toByte()
            payload[2] = ((idx ushr 8) and 0xFF).toByte()
            payload[3] = (idx and 0xFF).toByte()
            System.arraycopy(g, 0, payload, 4, g.size)
            out.add(payload)
        }
        return out
    }
}

/** Reassembles `[total][idx][data]` chunks into the full PNG. */
class ClipboardImageAssembler {
    private var total = 0
    private var chunks: Array<ByteArray?> = arrayOf()

    /** Feed a chunk payload; returns the full PNG once all chunks arrive. */
    fun add(payload: ByteArray): ByteArray? {
        if (payload.size < 4) return null
        val t = ((payload[0].toInt() and 0xFF) shl 8) or (payload[1].toInt() and 0xFF)
        val idx = ((payload[2].toInt() and 0xFF) shl 8) or (payload[3].toInt() and 0xFF)
        if (t == 0 || t > 4096 || idx >= t) return null
        if (total != t) {
            total = t
            chunks = arrayOfNulls(t)
        }
        chunks[idx] = payload.copyOfRange(4, payload.size)
        if (chunks.any { it == null }) return null
        val out = java.io.ByteArrayOutputStream()
        for (c in chunks) out.write(c!!)
        total = 0
        chunks = arrayOf()
        return out.toByteArray()
    }
}
