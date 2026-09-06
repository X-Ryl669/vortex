package com.vortex.a3.core.fs

import android.util.Log
import com.vortex.a3.core.ble.FrameType
import java.io.File
import java.io.RandomAccessFile
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject

/**
 * The phone's end of the ranged-filesystem protocol as a CONSUMER: browsing the
 * laptop's shared folders and pulling files off it.
 *
 * The mirror image of [FsServer], and the counterpart of Rust `fs_link`'s
 * client half. The protocol is symmetric, so this needs no new frames — it
 * sends the same FS_REQ ops the laptop sends us and reads the same replies.
 *
 * Requests are pipelined and correlated by id: a browse issues a listing while
 * a download is still running, and replies may arrive in any order.
 *
 * Transport is BLE. The laptop prefers Wi-Fi for the same traffic in the other
 * direction, but it can do that because the phone LISTENS on TCP and the laptop
 * dials it; there is no listener the other way, so a phone-initiated LAN
 * session has nothing to connect to. Listings are small and fine over BLE;
 * pulling a large file this way is slow, and closing that gap means giving the
 * laptop a listener.
 */
object FsClient {

    /** Failure of one request, errno-shaped so callers can say something true.
     *  [FsCode] values, or [TIMEOUT] when the laptop never answered. */
    class FsException(val code: Int, message: String) : Exception(message)

    /** No reply inside [REQUEST_TIMEOUT_MS]. Distinct from any server code so
     *  "the laptop went away" never reads as "the file is missing". */
    const val TIMEOUT = -1

    /**
     * How long a request waits. Generous because the far side may be reading a
     * cold disk, but finite: a UI blocked forever on a laptop that went to
     * sleep is this feature's worst outcome.
     */
    private const val REQUEST_TIMEOUT_MS = 20_000L

    private const val TAG = "VortexFs"

    /** Set by VortexStack: sends one FS_REQ, returns false if there is no link. */
    @Volatile
    var sender: ((op: Byte, payload: ByteArray) -> Boolean)? = null

    private val lock = Any()
    private var nextId = 1
    private val inflight = HashMap<Int, CompletableDeferred<Reply>>()

    private sealed class Reply {
        data class Meta(val json: JSONObject) : Reply()
        data class Data(val d: FsData) : Reply()
        data class Err(val e: FsErr) : Reply()
    }

    /** Feed a reply frame in. Wired to `GattServer.onFsReply`. */
    fun onReply(frameType: Byte, payload: ByteArray) {
        val reply: Reply = when (frameType) {
            FrameType.FS_DATA -> Reply.Data(decodeData(payload) ?: run {
                Log.w(TAG, "fs: truncated FS_DATA")
                return
            })
            FrameType.FS_ERR -> {
                val o = runCatching { JSONObject(String(payload, Charsets.UTF_8)) }.getOrNull()
                    ?: return
                Reply.Err(FsErr.from(o))
            }
            FrameType.FS_META -> {
                val o = runCatching { JSONObject(String(payload, Charsets.UTF_8)) }.getOrNull()
                    ?: return
                Reply.Meta(o)
            }
            else -> return
        }
        val id = when (reply) {
            is Reply.Data -> reply.d.id
            is Reply.Err -> reply.e.id
            is Reply.Meta -> reply.json.optInt("id")
        }
        val waiter = synchronized(lock) { inflight.remove(id) }
        if (waiter == null) {
            // A reply to a request that already timed out, or an id we never
            // issued. Dropped, but logged: silently ignoring these hides a
            // desynchronised protocol.
            Log.i(TAG, "fs: reply for unknown id=$id")
            return
        }
        waiter.complete(reply)
    }

    /** Drop every waiter — the link went, so nothing in flight can be answered. */
    fun reset() {
        val waiters = synchronized(lock) {
            val all = inflight.values.toList()
            inflight.clear()
            all
        }
        // Fail them rather than leaving callers parked on the timeout: the
        // answer is already known.
        waiters.forEach { it.complete(Reply.Err(FsErr(0, FsCode.IO, "link went away"))) }
    }

    private suspend fun roundTrip(op: Byte, id: Int, payload: ByteArray): Reply {
        val d = CompletableDeferred<Reply>()
        synchronized(lock) { inflight[id] = d }
        val send = sender
        if (send == null || !send(op, payload)) {
            synchronized(lock) { inflight.remove(id) }
            throw FsException(FsCode.IO, "no link to the laptop")
        }
        val reply = withTimeoutOrNull(REQUEST_TIMEOUT_MS) { d.await() }
        if (reply == null) {
            synchronized(lock) { inflight.remove(id) }
            throw FsException(TIMEOUT, "the laptop did not answer")
        }
        if (reply is Reply.Err) throw FsException(reply.e.code, reply.e.msg)
        return reply
    }

    private fun newId(): Int = synchronized(lock) {
        // Wrapping is fine: ids only need to be unique among what is in flight,
        // and 0 is reserved for "no particular request" in FS_ERR.
        nextId += 1
        if (nextId <= 0) nextId = 1
        nextId
    }

    /** One page of a directory. Empty path is the laptop's synthetic root. */
    suspend fun list(path: String, cursor: Int = 0): Pair<List<FsEntry>, Int?> {
        val id = newId()
        val r = roundTrip(FsOp.LIST, id, ListReq(id, path, cursor).toJson().toString().toByteArray())
        val o = (r as? Reply.Meta)?.json ?: throw FsException(FsCode.IO, "unexpected reply")
        val arr = o.optJSONArray("entries")
        val out = ArrayList<FsEntry>(arr?.length() ?: 0)
        for (i in 0 until (arr?.length() ?: 0)) out.add(FsEntry.from(arr!!.getJSONObject(i)))
        val next = if (o.has("cursor") && !o.isNull("cursor")) o.optInt("cursor") else null
        return out to next
    }

    /** Every page of a directory, followed to the end. */
    suspend fun listAll(path: String): List<FsEntry> {
        val out = ArrayList<FsEntry>()
        var cursor: Int? = 0
        var pages = 0
        while (cursor != null) {
            val (page, next) = list(path, cursor)
            out.addAll(page)
            cursor = next
            // A peer that keeps handing back a cursor without advancing would
            // loop us forever; stop rather than spin.
            if (++pages > 1000) break
        }
        return out
    }

    /**
     * Download [path] to [dest], streaming in ranges.
     *
     * Peak memory is one chunk however big the file is — the same property that
     * removed the transfer size cap in the other direction. [onProgress] gets
     * bytes-so-far and the total (or -1 when unknown).
     */
    suspend fun download(
        path: String,
        dest: File,
        onProgress: (done: Long, total: Long) -> Unit = { _, _ -> },
    ): File {
        val openId = newId()
        val opened = roundTrip(
            FsOp.OPEN,
            openId,
            OpenReq(openId, path).toJson().toString().toByteArray(),
        )
        val o = (opened as? Reply.Meta)?.json ?: throw FsException(FsCode.IO, "unexpected reply")
        val handle = o.optLong("handle")
        val size = o.optLong("size", -1)

        dest.parentFile?.mkdirs()
        var offset = 0L
        try {
            RandomAccessFile(dest, "rw").use { out ->
                out.setLength(0)
                while (true) {
                    val id = newId()
                    val r = roundTrip(
                        FsOp.READ,
                        id,
                        ReadReq(id, handle, offset, MAX_READ_LEN).toJson().toString().toByteArray(),
                    )
                    val d = (r as? Reply.Data)?.d
                        ?: throw FsException(FsCode.IO, "expected data")
                    if (d.bytes.isNotEmpty()) {
                        // Seek to the offset we were given rather than
                        // appending: the reply carries one so a reader that
                        // pipelines later cannot write bytes out of order.
                        out.seek(d.offset)
                        out.write(d.bytes)
                        offset = d.offset + d.bytes.size
                        onProgress(offset, size)
                    }
                    if (d.eof) break
                    if (d.bytes.isEmpty()) {
                        // No EOF and no bytes: the far side is not advancing,
                        // and retrying would spin forever.
                        throw FsException(FsCode.IO, "transfer stalled")
                    }
                }
            }
        } catch (e: Exception) {
            // No half-written file left behind: a truncated download looks like
            // a real one and is worse than none.
            dest.delete()
            closeQuietly(handle)
            throw e
        }
        closeQuietly(handle)
        return dest
    }

    private suspend fun closeQuietly(handle: Long) {
        try {
            val id = newId()
            roundTrip(FsOp.CLOSE, id, CloseReq(id, handle).toJson().toString().toByteArray())
        } catch (e: Exception) {
            // The far side expires idle handles anyway; failing to close is not
            // worth failing a completed download over.
            Log.i(TAG, "fs: close failed harmlessly: ${e.message}")
        }
    }
}
