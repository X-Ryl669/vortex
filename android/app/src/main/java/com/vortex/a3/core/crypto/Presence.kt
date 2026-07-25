package com.vortex.a3.core.crypto

import java.nio.ByteBuffer

/** Private presence token per spec §7.3. */
object Presence {
    val LABEL: ByteArray = "vortex/v1/presence".toByteArray(Charsets.UTF_8)
    const val TOKEN_LEN: Int = 8

    /** 8-byte rotating token derived from a peer's PRS and the current bucket. */
    fun deriveToken(prs: ByteArray, bucket: Long): ByteArray {
        val data = ByteBuffer.allocate(LABEL.size + 8)
            .put(LABEL)
            .putLong(bucket)
            .array()
        return Hmac.sha256(prs, data).copyOfRange(0, TOKEN_LEN)
    }

    fun currentBucket(unixSeconds: Long, rotationWindowSeconds: Long): Long {
        require(rotationWindowSeconds > 0) { "rotation window must be > 0" }
        return unixSeconds / rotationWindowSeconds
    }
}
