package com.vortex.a3.core.crypto

/** PRS and SES derivations per spec §6.5.2 and §6.5.3. */
object Derive {
    val PRS_LABEL: ByteArray = "vortex/v1/prs".toByteArray(Charsets.UTF_8)
    val SES_LABEL: ByteArray = "vortex/v1/ses".toByteArray(Charsets.UTF_8)

    /** PRS = HMAC-SHA256(label, ck). 32 bytes. */
    fun prs(chainingKey: ByteArray): ByteArray = Hmac.sha256(PRS_LABEL, chainingKey)

    /** SES = HMAC-SHA256(label, h). 32 bytes. Per-session, not persisted. */
    fun ses(transcriptHash: ByteArray): ByteArray = Hmac.sha256(SES_LABEL, transcriptHash)
}
