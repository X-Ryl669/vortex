package com.vortex.a3.privacy

import com.vortex.a3.core.crypto.Derive
import com.vortex.a3.core.crypto.Presence
import com.vortex.a3.core.crypto.Sas
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNotEquals
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import kotlin.test.fail
import java.io.File

/**
 * Phase 9 — log redaction gate (spec §3.5 T-LOC-3).
 *
 * Two complementary checks, mirroring the Rust gate in
 * `l3/daemon/tests/log_redaction.rs`:
 *
 *   1. Static source scan over `app/src/main/java/com/vortex/a3` for any
 *      `Log.<level>(TAG, "...")` site that formats one of the V1 secret
 *      values (PRS, SES, SAS code, raw presence token, static_priv).
 *
 *   2. Runtime smoke test: derive synthetic PRS / SES / SAS / presence
 *      tokens and verify (a) the derived bytes are non-empty and
 *      (b) presence tokens rotate per bucket (the non-linkability
 *      property cited in the build plan).
 *
 * The static scan is the production gate — the runtime smoke ensures the
 * primitives keep working as the codebase evolves.
 */
class LogRedactionTest {

    // ----------------------------------------------------------------
    // 1. Static source scan
    // ----------------------------------------------------------------

    @Test
    fun `no Log call writes a V1 secret`() {
        val mainDir = locateMainDir()
        val violations = mutableListOf<String>()

        mainDir.walkTopDown()
            .filter { it.isFile && it.name.endsWith(".kt") }
            .forEach { file ->
                val text = file.readText()
                val lines = text.lines()
                lines.forEachIndexed { idx, raw ->
                    val line = raw.trimStart()
                    if (line.startsWith("//") || line.startsWith("*")) return@forEachIndexed
                    if (!LOG_SINKS.any { line.contains(it) }) return@forEachIndexed

                    // Documented escape hatch: a `// LOG_REDACTION_ALLOW: <reason>`
                    // marker on the same line OR the immediately preceding line
                    // whitelists this site (used for debug-only paths gated by
                    // a build flag — see spec §3.5 T-LOC-3).
                    if (raw.contains(ALLOW_MARKER)) return@forEachIndexed
                    val prevLineAllowed =
                        idx > 0 && lines[idx - 1].contains(ALLOW_MARKER)
                    if (prevLineAllowed) return@forEachIndexed

                    FORBIDDEN_TOKENS.forEach { (token, doc) ->
                        if (line.contains(token)) {
                            violations += "${file.relativeTo(mainDir)}:${idx + 1} forbidden ($doc): ${line.trim()}"
                        }
                    }
                }
            }

        if (violations.isNotEmpty()) {
            fail(
                "log redaction gate (spec §3.5 T-LOC-3) failed; " +
                    "${violations.size} site(s):\n  " + violations.joinToString("\n  ")
            )
        }
    }

    // ----------------------------------------------------------------
    // 2. Runtime smoke — primitives produce well-formed secrets
    // ----------------------------------------------------------------

    @Test
    fun `derived secrets are non-empty and well-formed`() {
        val transcript = ByteArray(32) { it.toByte() }
        val prs = Derive.prs(transcript)
        val ses = Derive.ses(transcript)
        val (sasInt, sasStr) = Sas.derive(transcript)

        assertTrue(prs.size == 32, "PRS must be 32 bytes; was ${prs.size}")
        assertTrue(ses.size == 32, "SES must be 32 bytes; was ${ses.size}")
        assertTrue(sasStr.length == 6, "SAS string must be 6 digits; was '$sasStr'")
        assertTrue(sasInt in 0..999_999, "SAS int out of range: $sasInt")
        assertNotEquals(prs.toList(), ses.toList(), "PRS and SES collided")

        // Deriving the same input twice yields the same output.
        val prs2 = Derive.prs(transcript)
        assertTrue(prs.contentEquals(prs2), "Derive.prs is non-deterministic")
    }

    @Test
    fun `presence tokens rotate per bucket and do not leak PRS`() {
        // spec §7.3 — adjacent buckets MUST produce different tokens, and
        // the token MUST NOT contain the PRS bytes verbatim.
        val prs = ByteArray(32) { 0xAA.toByte() }
        val t0 = Presence.deriveToken(prs, 1_000L)
        val tPrev = Presence.deriveToken(prs, 999L)
        val tNext = Presence.deriveToken(prs, 1_001L)

        assertFalse(t0.contentEquals(tPrev), "presence token did not rotate (prev)")
        assertFalse(t0.contentEquals(tNext), "presence token did not rotate (next)")
        assertFalse(tPrev.contentEquals(tNext), "adjacent buckets collide")

        val prsHex = prs.toHex()
        val tokenHex = t0.toHex()
        assertFalse(
            tokenHex.contains(prsHex),
            "presence token leaks PRS bytes verbatim",
        )
    }

    // ----------------------------------------------------------------
    // helpers
    // ----------------------------------------------------------------

    private fun locateMainDir(): File {
        var dir = File(System.getProperty("user.dir"))
        repeat(6) {
            val candidate = File(dir, "app/src/main/java/com/vortex/a3")
            if (candidate.isDirectory) return candidate
            dir = dir.parentFile ?: return@repeat
        }
        error("could not locate app/src/main/java/com/vortex/a3 from ${System.getProperty("user.dir")}")
    }

    private fun ByteArray.toHex(): String =
        joinToString("") { "%02x".format(it) }

    companion object {
        /** Lines containing any of these tokens are treated as log sinks. */
        private val LOG_SINKS = listOf(
            "Log.v",
            "Log.d",
            "Log.i",
            "Log.w",
            "Log.e",
            "Log.wtf",
            "println(",
            "print(",
        )

        /**
         * Forbidden formatting tokens. Each entry is (token, docs-reference).
         * The token is matched as a substring on the same line as a log sink.
         * Patterns are conservative: false positives are caught by the
         * documented escape hatch (see [ALLOW_MARKER]).
         */
        private val FORBIDDEN_TOKENS = listOf(
            "prs.toHex" to "spec §3.5 T-LOC-3 (PRS)",
            "prs.joinToString" to "spec §3.5 T-LOC-3 (PRS)",
            "\$prs " to "spec §3.5 T-LOC-3 (PRS)",
            "ses.toHex" to "spec §3.5 T-LOC-3 (SES)",
            "ses.joinToString" to "spec §3.5 T-LOC-3 (SES)",
            "staticPriv.toHex" to "spec §3.5 T-LOC-3 (static_priv)",
            "staticPriv.joinToString" to "spec §3.5 T-LOC-3 (static_priv)",
            "\$sasString" to "spec §3.5 T-LOC-3 (SAS code)",
            "sasString)" to "spec §3.5 T-LOC-3 (SAS code)",
            "presenceToken.toHex" to "spec §3.5 T-LOC-3 (presence token)",
            "presenceToken.joinToString" to "spec §3.5 T-LOC-3 (presence token)",
        )

        private const val ALLOW_MARKER = "LOG_REDACTION_ALLOW"
    }
}
