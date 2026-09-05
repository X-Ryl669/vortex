package com.vortex.a3.service

import com.vortex.a3.core.ble.FrameType
import com.vortex.a3.core.fs.FsHandles
import com.vortex.a3.core.fs.FsRoots
import com.vortex.a3.core.fs.FsServer
import com.vortex.a3.core.fs.FsCode
import com.vortex.a3.core.fs.FsErr
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

/**
 * Wires filesystem serving (FS_REQ 0x50 → FS_META / FS_DATA / FS_ERR) into the
 * BLE stack: the phone half of `docs/design/file-browsing.md`, answering the
 * laptop's ranged reads against the folders the user has shared.
 *
 * Serving is READ-ONLY in v1. Writes are refused explicitly rather than
 * dropped — see [FsServer].
 *
 * Extension fn on [VortexStack]; call once after the GATT server is up.
 */
internal fun VortexStack.startFsServer() {
    val roots = FsRoots(ctx)
    val handles = FsHandles()
    val server = FsServer(ctx, roots, handles)
    fsHandles = handles

    gattServer?.onFsRequest = { peerPub, op, payload ->
        // Off the GATT callback thread, always. A document provider can stall
        // for seconds — a cloud-backed one indefinitely — and blocking here
        // would stall every other frame on the link behind one slow folder,
        // including the audio-switch path that shares this characteristic.
        scope.launch(Dispatchers.IO) {
            val srv = try {
                server.serve(op, payload)
            } catch (e: Exception) {
                // Never let an unexpected provider exception become silence:
                // the laptop is blocked on this request id and would wait out
                // its timeout instead of showing an error.
                FsServer.Served.Err(FsErr(0, FsCode.IO, e.message ?: "serve failed"))
            }
            val (type, bytes) = when (srv) {
                is FsServer.Served.Meta -> FrameType.FS_META to srv.reply.toJsonBytes()
                is FsServer.Served.Data -> FrameType.FS_DATA to srv.bytes
                is FsServer.Served.Err -> FrameType.FS_ERR to srv.err.toJsonBytes()
            }
            gattServer?.sendFsReply(peerPub, type, bytes)
        }
    }
}

/** Drop every open handle for a link that has gone. Handles cannot outlive the
 *  session that owns them: the ids are only meaningful to that peer, and an
 *  abandoned descriptor is a leak the idle sweep would take five minutes to
 *  notice. */
internal fun VortexStack.stopFsServer() {
    fsHandles?.clear()
}
