package com.vortex.a3.service

import java.security.MessageDigest

/**
 * Small shared helpers for the [VortexStack] feature files (split out of the
 * former god-class). Package-scoped (`internal`) so every `VortexStack.*.kt`
 * extension file can use them without re-declaring. Same names/bodies as the
 * old private members, so call sites are unchanged.
 */

/** Full lowercase-hex of a byte array (peer static pub, outbox key, …). */
internal fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }

/** First 4 bytes as hex + ellipsis — for terse log lines. */
internal fun ByteArray.toHexPrefix(): String =
    take(4).joinToString("") { "%02x".format(it) } + "…"

/** Alias kept for the notification-outbox key call sites. */
internal fun ByteArray.notifHex(): String = toHex()

/** SHA-256 of bytes as lowercase hex (companion-mirror LAN bulk-sync keys). */
internal fun sha256Hex(bytes: ByteArray): String =
    MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
