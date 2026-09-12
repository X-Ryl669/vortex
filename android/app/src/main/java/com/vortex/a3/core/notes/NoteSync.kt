package com.vortex.a3.core.notes

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Bidirectional notes/todos sync (NOTES_SYNC 0x4D) — the phone half of the
 * state-based LWW-Element-Set in the laptop's `notes.rs`. Pushes the FULL item
 * set to the peer after a local edit / on (re)connect; on an inbound set,
 * LWW-merges by `updatedAt` (+ tombstones) and replies ONLY if we hold items the
 * peer lacked — converges in ≤2 rounds. BLE-only (chunked over AUDIO_SIGNAL);
 * the merge + connect-push keep it eventually consistent across drops.
 */
object NoteSync {
    private const val CHUNK_DATA = 400 // fits one BLE notify after AEAD

    /** Sends one chunk to the peer (set by the wiring; BLE notify). */
    @Volatile var sendChunk: ((ByteArray) -> Unit)? = null

    private var scope: CoroutineScope? = null
    private var dirtyJob: Job? = null
    private val asm = Assembler()

    fun init(scope: CoroutineScope) {
        this.scope = scope
    }

    /** LWW union by id: keep the greater `updatedAt` (a tombstone propagates). */
    fun merge(local: List<Note>, remote: List<Note>): List<Note> {
        val byId = HashMap<String, Note>()
        for (n in local + remote) {
            val cur = byId[n.id]
            if (cur == null || n.updatedAt > cur.updatedAt) byId[n.id] = n
        }
        return byId.values.toList()
    }

    /** Canonical signature (sorted id:updatedAt:deleted) — order-independent eq. */
    fun sig(items: List<Note>): String =
        items.map { "${it.id}:${it.updatedAt}:${it.deleted}" }.sorted().joinToString("|")

    /** Do WE hold anything the peer's set does not — an item they lack, or a
     *  newer version of one they have?
     *
     *  This is the reply condition, and it used to be `sig(merged) != sig(remote)`,
     *  which was an equivalent proxy only while neither side ever dropped
     *  anything. Once tombstones expire it is not: a peer still holding one we
     *  have already dropped makes the signatures differ for ever, and answering
     *  that difference is an endless exchange — they send it, we merge and
     *  re-prune, reply, they reply. Which is exactly the window where one device
     *  has updated and the other has not.
     *
     *  Asking the question the comment always claimed to ask — do they need
     *  anything FROM US — makes a set that is merely smaller than theirs silent,
     *  while still replying whenever we genuinely have something to contribute. */
    fun weHoldMore(ours: List<Note>, remote: List<Note>): Boolean {
        val theirs = HashMap<String, Long>(remote.size)
        for (r in remote) theirs[r.id] = r.updatedAt
        return ours.any { o ->
            val t = theirs[o.id]
            t == null || o.updatedAt > t
        }
    }

    fun buildChunks(items: List<Note>): List<ByteArray> {
        val json = Note.listToBytes(items)
        val total = ((json.size + CHUNK_DATA - 1) / CHUNK_DATA).coerceAtLeast(1)
        return (0 until total).map { i ->
            val start = i * CHUNK_DATA
            val end = minOf(start + CHUNK_DATA, json.size)
            val data = json.copyOfRange(start, end)
            val f = ByteArray(4 + data.size)
            f[0] = (total ushr 8).toByte(); f[1] = total.toByte()
            f[2] = (i ushr 8).toByte(); f[3] = i.toByte()
            data.copyInto(f, 4)
            f
        }
    }

    private fun parseChunk(p: ByteArray): Triple<Int, Int, ByteArray>? {
        if (p.size < 4) return null
        val total = ((p[0].toInt() and 0xFF) shl 8) or (p[1].toInt() and 0xFF)
        val idx = ((p[2].toInt() and 0xFF) shl 8) or (p[3].toInt() and 0xFF)
        return Triple(total, idx, p.copyOfRange(4, p.size))
    }

    /** Reassembles a chunked full item set. */
    private class Assembler {
        private val parts = sortedMapOf<Int, ByteArray>()
        private var lastTotal = -1

        @Synchronized
        fun add(total: Int, idx: Int, data: ByteArray): List<Note>? {
            if (total <= 0 || total > 4096) return null
            // A different `total` means a DIFFERENT set — the previous one never
            // finished. Keeping its parts mixed old chunks into the new set and
            // produced a buffer that was neither, so after a single dropped
            // chunk the assembler stayed wrong until two pushes happened to
            // share a chunk count.
            if (total != lastTotal) {
                parts.clear()
                lastTotal = total
            }
            // Restarting at index 0 is the same signal: a fresh push of the
            // same size.
            if (idx == 0 && parts.isNotEmpty()) parts.clear()
            parts[idx] = data
            if (parts.size != total) return null
            val buf = parts.values.fold(ByteArray(0)) { a, b -> a + b }
            parts.clear()
            lastTotal = -1
            return Note.listFromBytes(buf)
        }
    }

    /** Send our full set to the peer (chunked). */
    fun sendFull(items: List<Note>) {
        val s = sendChunk ?: return
        val sc = scope
        val chunks = buildChunks(items)
        if (chunks.size <= 1 || sc == null) {
            chunks.forEach { s(it) }
            return
        }
        // Pace it. This went out back-to-back, which is exactly what the GATT
        // server's own comment warns about ("back-to-back notifies overflow the
        // stack's queue and drop") and what the clipboard path already spaces at
        // 12 ms. Any set over ~400 bytes — two or three notes — was riding the
        // unpaced path, and a single dropped chunk left the laptop's assembler
        // waiting for a piece that never comes.
        sc.launch {
            for ((i, c) in chunks.withIndex()) {
                if (i > 0) delay(12)
                s(c)
            }
        }
    }

    /** Debounced push of our full set after a local edit / on connect. */
    fun markDirty() {
        val sc = scope ?: return
        dirtyJob?.cancel()
        dirtyJob = sc.launch {
            delay(300)
            sendFull(NoteStore.snapshot())
        }
    }

    /** Handle an inbound NOTES_SYNC chunk: reassemble → LWW merge → persist, and
     *  reply only if WE hold items the peer's set lacked (→ converges). */
    fun onInbound(payload: ByteArray) {
        val (total, idx, data) = parseChunk(payload) ?: return
        val remote = asm.add(total, idx, data) ?: return
        val before = NoteStore.snapshot()
        // Prune here too, not only in the store: `merged` is what we reply with,
        // and re-sending a tombstone we have just expired would hand it straight
        // back to us on the next round.
        val merged = NoteStore.prune(merge(before, remote))
        if (sig(merged) != sig(before)) {
            NoteStore.replaceAll(merged) // persist + publish (no echo)
        }
        if (weHoldMore(merged, remote)) {
            sendFull(merged)
        }
    }
}
