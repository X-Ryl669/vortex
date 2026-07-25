package com.vortex.a3.core.lan

import android.util.Log
import com.southernstorm.noise.protocol.CipherState
import com.southernstorm.noise.protocol.CipherStatePair
import com.vortex.a3.core.ble.FRAME_HEADER_LEN
import com.vortex.a3.core.ble.Frame
import com.vortex.a3.core.ble.MAX_FRAME_PAYLOAD
import org.json.JSONObject
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.Socket

/**
 * Phone side of the dedicated SCREEN-MIRRORING Noise session.
 *
 * [LanServer.handleClient] runs the normal IK handshake, then peeks the first
 * post-handshake frame. If its type is [SCREEN_MIRROR_CONTROL] it hands the
 * already-split [CipherStatePair] + the socket here, and this class owns the
 * connection for the rest of the mirror session.
 *
 * Channel split (mirrors the Linux `core/mirror_session.rs`):
 *  - **This TCP session** carries the reliable control plane (START / STOP /
 *    request-keyframe / ping-pong) and **laptop → phone input events**.
 *  - The H.264 **video** rides a separate **UDP** data-plane (M2), keyed by
 *    HKDF over [handshakeHash]; UDP so a lost packet never stalls the stream.
 *
 * Concurrency: exactly ONE coroutine drives [run] — it is the sole reader of
 * `pair.receiver` and the sole writer of `pair.sender` (the only thing written
 * back over TCP is a PONG), so the per-direction Noise nonces stay monotone
 * without an extra lock (same invariant LanServer documents for its own loop).
 *
 * Frame-type bytes are defined locally for now; they migrate into the shared
 * `FrameType` table (`SCREEN_MIRROR_CONTROL = 0x47`, `SCREEN_MIRROR_INPUT =
 * 0x46`) when that edit lands. `Frame` carries a raw `Byte` type, so this is
 * wire-compatible in the meantime.
 */
class MirrorSession(
    private val sock: Socket,
    private val input: DataInputStream,
    private val output: DataOutputStream,
    private val pair: CipherStatePair,
    /** IK handshake hash — derive the UDP media key from this (M2). */
    val handshakeHash: ByteArray,
    /** Called once with the laptop's requested encoder params (start capture). */
    private val onStart: (MirrorStart) -> Unit,
    /** Called with each 5-byte laptop→phone input packet (→ injector, M3). */
    private val onInput: (ByteArray) -> Unit,
    /** Laptop asked for an IDR (UDP frame loss) → encoder emits a keyframe. */
    private val onRequestKeyframe: () -> Unit,
    /** Session ended (STOP / EOF / error) → stop capture + release. */
    private val onStop: () -> Unit,
) {
    /** The remote (laptop) IP — where the phone sends UDP video (M2). */
    val peerAddress: java.net.InetAddress = sock.inetAddress

    /**
     * Drive the mirror session. [firstFrame] is the SCREEN_MIRROR_CONTROL/START
     * frame the LanServer already read (header inspected, payload still sealed).
     * Blocks until the session ends; call from the existing client coroutine.
     */
    fun run(firstFrame: Frame) {
        try {
            // The first frame must be START — open it (first use of receiver).
            if (firstFrame.type != SCREEN_MIRROR_CONTROL || firstFrame.sub != SUB_START) {
                Log.w(TAG, "mirror: first frame not START; closing")
                return
            }
            val startPlain = try {
                aeadOpen(pair.receiver, firstFrame.payload)
            } catch (e: Exception) {
                Log.w(TAG, "mirror: START AEAD open failed: ${e.message}")
                return
            }
            val start = MirrorStart.fromJson(startPlain) ?: run {
                Log.w(TAG, "mirror: bad START payload")
                return
            }
            Log.i(TAG, "mirror: START ${start.w}x${start.h}@${start.fps} ${start.bitrate}bps udp=${start.udpPort}")
            onStart(start)

            // Control loop: input + ping/keyframe/stop. Single writer (PONG).
            while (true) {
                val frame = readFrame(input) ?: break
                // The laptop SEALS every frame it sends — including the empty
                // control payloads (keepalive ping, keyframe-req, stop). Each
                // seal advances its sender nonce, so we MUST open every frame to
                // keep our receiver nonce in lockstep. Opening only INPUT (and
                // skipping ping/keyframe) desyncs the AEAD the instant the first
                // ping or keyframe-req arrives, and then every later INPUT fails
                // to decrypt — i.e. control "randomly stops working".
                val plain = try {
                    aeadOpen(pair.receiver, frame.payload)
                } catch (e: Exception) {
                    Log.w(TAG, "mirror: AEAD open failed (type=0x${"%02x".format(frame.type)}): ${e.message}")
                    continue
                }
                when (frame.type) {
                    SCREEN_MIRROR_INPUT -> if (plain.size == 5) onInput(plain)
                    SCREEN_MIRROR_CONTROL -> when (frame.sub) {
                        SUB_PING -> sealAndWrite(SCREEN_MIRROR_CONTROL, SUB_PONG, ByteArray(0))
                        SUB_REQUEST_KEYFRAME -> onRequestKeyframe()
                        SUB_STOP -> {
                            Log.i(TAG, "mirror: STOP received")
                            return
                        }
                        else -> Log.d(TAG, "mirror: control sub=0x${"%02x".format(frame.sub)}")
                    }
                    else -> Log.d(TAG, "mirror: unexpected frame type=0x${"%02x".format(frame.type)}")
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "mirror: session error: ${e.message}")
        } finally {
            onStop()
            try { sock.close() } catch (_: Exception) {}
            Log.i(TAG, "mirror: session closed")
        }
    }

    // ---- frame + AEAD helpers (self-contained; mirror LanServer's privates) ----

    private fun sealAndWrite(type: Byte, sub: Byte, plaintext: ByteArray) {
        val ct = aeadSeal(pair.sender, plaintext)
        output.write(Frame(type, sub, ct).encode())
        output.flush()
    }

    private fun readFrame(input: DataInputStream): Frame? {
        return try {
            val header = ByteArray(FRAME_HEADER_LEN)
            input.readFully(header)
            val length = ((header[2].toInt() and 0xFF) shl 8) or (header[3].toInt() and 0xFF)
            if (length > MAX_FRAME_PAYLOAD) return null
            val payload = ByteArray(length)
            if (length > 0) input.readFully(payload)
            Frame.decode(header + payload).getOrNull()
        } catch (e: Exception) {
            null
        }
    }

    private fun aeadSeal(cipher: CipherState, plaintext: ByteArray): ByteArray {
        val out = ByteArray(plaintext.size + cipher.macLength)
        val n = cipher.encryptWithAd(null, plaintext, 0, out, 0, plaintext.size)
        return out.copyOf(n)
    }

    private fun aeadOpen(cipher: CipherState, ciphertext: ByteArray): ByteArray {
        if (ciphertext.size < cipher.macLength) {
            throw IllegalArgumentException("ciphertext shorter than MAC")
        }
        val out = ByteArray(ciphertext.size)
        val n = cipher.decryptWithAd(null, ciphertext, 0, out, 0, ciphertext.size)
        return out.copyOf(n)
    }

    companion object {
        private const val TAG = "VortexMirror"

        // TEMP local frame-type bytes — migrate into FrameType (0x47 / 0x46).
        const val SCREEN_MIRROR_INPUT: Byte = 0x46
        const val SCREEN_MIRROR_CONTROL: Byte = 0x47

        // SCREEN_MIRROR_CONTROL sub-codes (match core/mirror_session.rs::ctrl).
        const val SUB_START: Byte = 0x01
        const val SUB_STOP: Byte = 0x02
        const val SUB_REQUEST_KEYFRAME: Byte = 0x03
        const val SUB_PING: Byte = 0x04
        const val SUB_PONG: Byte = 0x05

        /**
         * True if the first post-handshake frame begins a mirror session, so
         * LanServer can branch to [MirrorSession] instead of its normal loop.
         */
        fun isMirrorStart(frame: Frame): Boolean =
            frame.type == SCREEN_MIRROR_CONTROL && frame.sub == SUB_START
    }
}

/** Encoder/stream parameters the laptop requests in the START control frame. */
data class MirrorStart(
    val w: Int,
    val h: Int,
    val fps: Int,
    val bitrate: Int,
    /** UDP port on the LAPTOP to stream H.264 video to. */
    val udpPort: Int,
) {
    companion object {
        fun fromJson(bytes: ByteArray): MirrorStart? = try {
            val o = JSONObject(String(bytes, Charsets.UTF_8))
            MirrorStart(
                w = o.getInt("w"),
                h = o.getInt("h"),
                fps = o.getInt("fps"),
                bitrate = o.getInt("bitrate"),
                udpPort = o.getInt("udp_port"),
            )
        } catch (e: Exception) {
            null
        }
    }
}
