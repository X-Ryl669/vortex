package com.vortex.a3.core.mirror

import java.nio.ByteBuffer
import java.nio.ByteOrder
import javax.crypto.Cipher
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Phone side of the screen-mirror **TCP** video data-plane (reliable + ordered,
 * like scrcpy). MUST stay byte-for-byte compatible with the Linux receiver in
 * `l3/daemon/src/core/mirror_tcp.rs`.
 *
 * Unlike the old UDP path there is no fragmentation or replay handling — TCP
 * delivers every byte in order, so each H.264 access unit is sealed whole and
 * written framed:
 * ```
 *   [ msg_len : u32 BE ]                 # = 8 + ciphertext length
 *   [ counter : u64 BE ]                 # nonce material + AAD
 *   [ ChaCha20-Poly1305(key, nonce = 0u32 || counter, aad = counter, AU) ]
 * ```
 *
 * Key = [MirrorUdp.deriveMediaKey] (the shared media key, derived from the IK
 * handshake hash). The `counter` is monotonic so the nonce never repeats.
 * Not thread-safe — drive from the single encoder-output thread.
 */
class MirrorTcpSealer(key: ByteArray) {
    private val keySpec = SecretKeySpec(key, "ChaCha20")
    private var counter: Long = 0

    /** Seal one access unit into a full framed message ready for `write()`. */
    fun sealAccessUnit(au: ByteArray): ByteArray {
        val counterVal = counter
        counter++
        val counterBytes = ByteBuffer.allocate(8).order(ByteOrder.BIG_ENDIAN)
            .putLong(counterVal).array()
        val nonce = ByteArray(12)
        System.arraycopy(counterBytes, 0, nonce, 4, 8) // high 4 bytes zero

        val cipher = Cipher.getInstance("ChaCha20-Poly1305")
        cipher.init(Cipher.ENCRYPT_MODE, keySpec, IvParameterSpec(nonce))
        cipher.updateAAD(counterBytes)
        val ct = cipher.doFinal(au) // ciphertext || 16-byte tag

        val msgLen = 8 + ct.size
        return ByteBuffer.allocate(4 + msgLen).order(ByteOrder.BIG_ENDIAN)
            .putInt(msgLen)
            .put(counterBytes)
            .put(ct)
            .array()
    }
}
