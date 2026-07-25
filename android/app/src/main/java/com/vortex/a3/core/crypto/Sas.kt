package com.vortex.a3.core.crypto

import java.nio.ByteBuffer

/** SAS derivation per spec §6.5.1. */
object Sas {
    val LABEL: ByteArray = "vortex/v1/sas".toByteArray(Charsets.UTF_8)

    /** Returns [sasValue (0..999_999), sasString (zero-padded 6 digits)]. */
    fun derive(transcriptHash: ByteArray): Pair<Int, String> {
        val mac = Hmac.sha256(LABEL, transcriptHash)
        val sasInt = ByteBuffer.wrap(mac, 0, 4).int.toLong() and 0xFFFFFFFFL
        val sasValue = (sasInt % 1_000_000L).toInt()
        return sasValue to "%06d".format(sasValue)
    }
}
