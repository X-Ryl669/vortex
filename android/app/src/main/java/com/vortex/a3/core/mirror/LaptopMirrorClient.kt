package com.vortex.a3.core.mirror

import android.media.MediaCodec
import android.media.MediaFormat
import android.util.Log
import android.view.Surface
import java.io.DataInputStream
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.nio.ByteBuffer
import java.nio.ByteOrder
import javax.crypto.Cipher
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Viewer side of the LAPTOP→phone screen mirror: connect to the laptop's sealed
 * H.264 video server, open each access unit (ChaCha20-Poly1305 — the exact
 * inverse of [MirrorTcpSealer]) and feed it to a MediaCodec H.264 decoder
 * rendering onto [surface]. The mirror image of the Linux SENDER in
 * `l3/daemon/src/core/mirror_tcp.rs` (`MirrorTcpSealer` + `run_tcp_video_server`).
 *
 * Codec is H.264 (AVC), CPU-encoded on the laptop (`x264enc`): GPU encode crashed
 * the laptop's Intel-driven compositor via a cross-GPU buffer import, so the
 * sender stays on the CPU. MediaCodec's AVC HW decoder handles it here.
 *
 * Wire (laptop → phone), repeated back-to-back:
 * ```
 *   [ msg_len u32 BE ][ counter u64 BE ][ ChaCha20-Poly1305(key, 0u32||counter, aad=counter, AU) ]
 * ```
 *
 * The media key is the laptop→phone key (random per cast, delivered over the
 * Noise-sealed control channel — NOT derived here). Drive [start] on a worker
 * thread; [stop] tears it down. Not reusable — make a new one per session.
 */
class LaptopMirrorClient(
    private val port: Int,
    key: ByteArray,
    private val surface: Surface,
    private val width: Int = 1280,
    private val height: Int = 720,
) {
    private val keySpec = SecretKeySpec(key, "ChaCha20")
    @Volatile private var running = false
    private var server: ServerSocket? = null
    private var socket: Socket? = null
    private var codec: MediaCodec? = null

    /** Blocking: accept the laptop's connection, decode + render until the
     *  stream ends or [stop]. The PHONE is the server here — on real networks
     *  only laptop→phone connections succeed, so the laptop dials us. */
    fun start() {
        running = true
        try {
            acceptAndDecode()
        } catch (t: Throwable) {
            if (running) Log.w(TAG, "laptop-mirror: stream ended: ${t.message}")
        } finally {
            cleanup()
        }
    }

    fun stop() {
        running = false
        try { socket?.close() } catch (_: Throwable) { /* unblocks the read */ }
        try { server?.close() } catch (_: Throwable) { /* unblocks accept() */ }
    }

    private fun acceptAndDecode() {
        // Bind + wait for the laptop to dial in (it starts capturing only after
        // the user approves the screen-share consent on the laptop, so allow a
        // generous accept window).
        val srv = ServerSocket()
        srv.reuseAddress = true
        srv.bind(InetSocketAddress(port))
        srv.soTimeout = 60_000
        server = srv
        Log.i(TAG, "laptop-mirror: viewer server up on :$port — waiting for laptop")

        // Accept loop: serve each laptop connection on a FRESH decoder, and
        // re-accept after a drop so a transient disconnect self-heals (the laptop
        // reconnects + resyncs on the next keyframe) instead of going black.
        while (running) {
            val s = try {
                srv.accept()
            } catch (t: Throwable) {
                if (running) Log.w(TAG, "laptop-mirror: no laptop connection: ${t.message}")
                return
            }
            s.tcpNoDelay = true
            socket = s
            Log.i(TAG, "laptop-mirror: laptop connected from ${s.inetAddress?.hostAddress}")
            try {
                decodeConnection(s)
            } catch (t: Throwable) {
                if (running) Log.w(TAG, "laptop-mirror: connection ended (${t.message}) — re-accepting")
            } finally {
                try { s.close() } catch (_: Throwable) {}
                socket = null
            }
        }
    }

    /** Decode one laptop connection on a fresh H.264 decoder until it drops or
     *  [stop] is called. SPS/PPS arrive in-band (config-interval=-1) so the
     *  decoder self-configures off the first keyframe. */
    private fun decodeConnection(s: Socket) {
        val input = DataInputStream(s.getInputStream().buffered())
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height)
        val dec = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        dec.configure(format, surface, null, 0)
        dec.start()
        codec = dec
        val info = MediaCodec.BufferInfo()
        val lenBuf = ByteArray(4)
        var frames = 0L
        // We connect mid-stream, so skip until the first KEYFRAME (SPS/IDR) — the
        // laptop emits one every ~1s (key-int-max) — else the decoder gets
        // un-decodable P-frames first and errors / shows garbage.
        var sawKeyframe = false
        try {
            while (running) {
                input.readFully(lenBuf)
                val msgLen = ByteBuffer.wrap(lenBuf).order(ByteOrder.BIG_ENDIAN).int
                if (msgLen < 8 + 16 || msgLen > MAX_AU) {
                    Log.w(TAG, "laptop-mirror: bad frame length $msgLen — closing")
                    break
                }
                val msg = ByteArray(msgLen)
                input.readFully(msg)
                val au = open(msg.copyOfRange(0, 8), msg)
                if (au == null) {
                    Log.w(TAG, "laptop-mirror: AEAD open failed — closing")
                    break
                }
                if (!sawKeyframe) {
                    if (!isKeyframe(au)) continue
                    sawKeyframe = true
                    Log.i(TAG, "laptop-mirror: first keyframe — decoding")
                }
                try {
                    val inIdx = dec.dequeueInputBuffer(10_000)
                    if (inIdx >= 0) {
                        val ib = dec.getInputBuffer(inIdx)
                        if (ib != null) {
                            ib.clear()
                            ib.put(au)
                            dec.queueInputBuffer(inIdx, 0, au.size, frames * 1_000, 0)
                        } else {
                            dec.queueInputBuffer(inIdx, 0, 0, frames * 1_000, 0)
                        }
                    }
                    var outIdx = dec.dequeueOutputBuffer(info, 0)
                    while (outIdx >= 0) {
                        dec.releaseOutputBuffer(outIdx, true) // render to the surface
                        outIdx = dec.dequeueOutputBuffer(info, 0)
                    }
                } catch (e: IllegalStateException) {
                    Log.w(TAG, "laptop-mirror: decoder hiccup: ${e.message}")
                    break
                }
                if (frames == 0L) Log.i(TAG, "laptop-mirror: first AU decoded")
                frames++
            }
        } finally {
            try { dec.stop() } catch (_: Throwable) {}
            try { dec.release() } catch (_: Throwable) {}
            if (codec === dec) codec = null
        }
    }

    /** True if this H.264 byte-stream access unit contains an SPS (NAL type 7)
     *  or IDR (type 5) — i.e. the decoder can start from it. Scans the Annex-B
     *  start codes (`00 00 01` / `00 00 00 01`) for the NAL header. */
    private fun isKeyframe(au: ByteArray): Boolean {
        var i = 0
        while (i + 3 < au.size) {
            if (au[i].toInt() == 0 && au[i + 1].toInt() == 0) {
                val nalIdx = when {
                    au[i + 2].toInt() == 1 -> i + 3
                    au[i + 2].toInt() == 0 && i + 3 < au.size && au[i + 3].toInt() == 1 -> i + 4
                    else -> { i++; continue }
                }
                if (nalIdx < au.size) {
                    val type = au[nalIdx].toInt() and 0x1F
                    if (type == 7 || type == 5) return true // SPS or IDR
                }
                i = nalIdx
            } else {
                i++
            }
        }
        return false
    }

    /** ChaCha20-Poly1305 open: nonce = 0u32 || counter, AAD = counter bytes. */
    private fun open(counterBytes: ByteArray, msg: ByteArray): ByteArray? = try {
        val nonce = ByteArray(12)
        System.arraycopy(counterBytes, 0, nonce, 4, 8) // high 4 bytes zero
        val cipher = Cipher.getInstance("ChaCha20-Poly1305")
        cipher.init(Cipher.DECRYPT_MODE, keySpec, IvParameterSpec(nonce))
        cipher.updateAAD(counterBytes)
        cipher.doFinal(msg, 8, msg.size - 8) // ciphertext||tag after the 8B counter
    } catch (_: Throwable) {
        null
    }

    private fun cleanup() {
        try { codec?.stop() } catch (_: Throwable) {}
        try { codec?.release() } catch (_: Throwable) {}
        try { socket?.close() } catch (_: Throwable) {}
        try { server?.close() } catch (_: Throwable) {}
        codec = null
        socket = null
        server = null
    }

    companion object {
        private const val TAG = "LaptopMirror"
        private const val MAX_AU = 8 * 1024 * 1024
    }
}
