package com.vortex.a3.core.crypto

import com.southernstorm.noise.protocol.Noise

/**
 * X25519 wrapper per spec §3 and §4.1.
 *
 * We reuse noise-java's `Noise.createDH("25519")` so the same X25519
 * implementation underlies pairing identities, Noise XX/IK handshakes,
 * and presence-token derivations.
 */
object X25519 {
    /** Private scalar size in bytes. */
    const val PRIV_LEN: Int = 32
    /** Public key size in bytes. */
    const val PUB_LEN: Int = 32

    /** Derive the X25519 public key from a 32-byte private scalar (RFC 7748). */
    fun publicFromPrivate(privateBytes: ByteArray): ByteArray {
        require(privateBytes.size == PRIV_LEN) { "X25519 private must be $PRIV_LEN bytes" }
        val dh = Noise.createDH("25519")
        try {
            dh.setPrivateKey(privateBytes, 0)
            val out = ByteArray(PUB_LEN)
            dh.getPublicKey(out, 0)
            return out
        } finally {
            dh.destroy()
        }
    }
}
