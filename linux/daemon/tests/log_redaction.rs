//! Phase 9 — log redaction gate (spec §3.5 T-LOC-3).
//!
//! Two complementary checks:
//!
//! 1. **Static source scan.** Walk `src/` and assert that no production
//!    `tracing` macro (`info!`, `warn!`, `error!`, `debug!`, `trace!`) or
//!    `println!`/`eprintln!` statement formats one of the V1 secret values.
//!    The grep is line-based and intentionally conservative; if a maintainer
//!    introduces a new secret-bearing log site, this test fails before any
//!    binary ships.
//!
//! 2. **Runtime tracing capture.** Drive the V1 derivation primitives with
//!    a known PRS / SES / SAS / presence token, capture all `tracing`
//!    output via a buffered `MakeWriter`, and assert that none of the
//!    secret bytes appear in the captured log buffer. This protects
//!    against a regression where a primitive starts logging its inputs.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing::{debug, error, info, warn};
use tracing_subscriber::fmt::MakeWriter;
use vortex_l3_daemon::core::crypto::derive::{derive_prs, derive_ses};
use vortex_l3_daemon::core::crypto::presence::derive_presence_token;
use vortex_l3_daemon::core::crypto::sas::derive_sas;

// --------------------------------------------------------------------------
// 1. Static source scan
// --------------------------------------------------------------------------

/// Patterns that, when paired with a log macro on the same physical line,
/// indicate a forbidden secret-bearing log site. Each entry is (token,
/// docs-reference) — the docs-reference is included in the failure message
/// so the maintainer can find the rule in `02-threat-model.md`.
const FORBIDDEN_TOKENS: &[(&str, &str)] = &[
    // PRS — Pairwise Reconnect Secret. Long-lived; never log.
    ("&outcome.prs", "spec §3.5 T-LOC-3 (PRS)"),
    ("(&prs", "spec §3.5 T-LOC-3 (PRS)"),
    ("(prs)", "spec §3.5 T-LOC-3 (PRS)"),
    // SES — Session Export Secret. Per-session; never log.
    ("(&ses", "spec §3.5 T-LOC-3 (SES)"),
    ("(ses)", "spec §3.5 T-LOC-3 (SES)"),
    // static_priv — long-lived X25519 secret. Never log.
    ("static_priv", "spec §3.5 T-LOC-3 (static_priv)"),
    // SAS code — short-lived, but per spec MUST NOT appear in logs.
    ("sas_string", "spec §3.5 T-LOC-3 (SAS code)"),
    // Raw presence token. Pseudonymous to peers but stable within
    // a rotation window; never log raw bytes.
    ("presence_token", "spec §3.5 T-LOC-3 (presence token)"),
];

/// Log-macro / stdout patterns that cause secret bytes to be written out.
/// `decide(&sas_string)` is allowed (it is the user-facing UI prompt
/// callback, not a log sink) so we narrow to the formatting macros.
const LOG_SINKS: &[&str] = &[
    "info!",
    "warn!",
    "error!",
    "debug!",
    "trace!",
    "println!",
    "eprintln!",
    "print!",
    "eprint!",
];

/// Source files that are *expected* to mention the secrets in non-log
/// contexts (struct fields, function bodies, etc.). The static scan
/// already filters by log-sink presence, but we additionally hard-skip
/// these tests / fixtures so the scan stays focused on production code.
const SCAN_SKIP: &[&str] = &[
    "tests/",   // this file and other integration tests
    "/bin/",    // vector_gen — emits secrets to JSON intentionally
];

#[test]
fn no_log_macro_writes_a_v1_secret() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations: Vec<String> = Vec::new();
    walk_rs_files(&src, &mut |path: &Path| {
        let path_str = path.to_string_lossy();
        if SCAN_SKIP.iter().any(|s| path_str.contains(s)) {
            return;
        }
        let contents = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let lines: Vec<&str> = contents.lines().collect();
        for (lineno, raw) in lines.iter().enumerate() {
            let line = raw.trim_start();
            // Skip comments — a `// PRS MUST NOT appear in logs` comment
            // is documentation, not a log site.
            if line.starts_with("//") || line.starts_with("///") {
                continue;
            }
            let has_log_sink = LOG_SINKS.iter().any(|m| line.contains(m));
            if !has_log_sink {
                continue;
            }
            // Documented escape hatch: a `// LOG_REDACTION_ALLOW: <reason>`
            // marker on the same line OR the immediately preceding line
            // whitelists this site. Used for debug-only paths gated by
            // `VORTEX_INSECURE_DEBUG=1` (see spec §3.5 T-LOC-3).
            let same_line_allow = raw.contains("LOG_REDACTION_ALLOW");
            let prev_line_allow = lineno
                .checked_sub(1)
                .and_then(|i| lines.get(i))
                .map(|l| l.contains("LOG_REDACTION_ALLOW"))
                .unwrap_or(false);
            if same_line_allow || prev_line_allow {
                continue;
            }
            for (token, doc) in FORBIDDEN_TOKENS {
                if line.contains(token) {
                    violations.push(format!(
                        "{}:{} forbidden ({}): {}",
                        path.display(),
                        lineno + 1,
                        doc,
                        line.trim(),
                    ));
                }
            }
        }
    });

    if !violations.is_empty() {
        panic!(
            "log redaction gate (spec §3.5 T-LOC-3) failed; {} site(s):\n  {}",
            violations.len(),
            violations.join("\n  "),
        );
    }
}

fn walk_rs_files(dir: &Path, visit: &mut dyn FnMut(&Path)) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs_files(&path, visit);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            visit(&path);
        }
    }
}

// --------------------------------------------------------------------------
// 2. Runtime tracing capture
// --------------------------------------------------------------------------

#[derive(Clone, Default)]
struct CapturingWriter(Arc<Mutex<Vec<u8>>>);

impl Write for CapturingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturingWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn primitives_do_not_emit_secrets_to_tracing() {
    // Known transcript hash (32 bytes) — fixed so we can recompute the
    // derived secrets and search for them in the captured log buffer.
    let transcript_hash = hex::decode(
        "a6f9b27ec9c5b0acf7df9c3c5c7d6c9e8a3b4d5e6f7080910a1b2c3d4e5f6071",
    )
    .expect("valid hex");
    let prs = derive_prs(&transcript_hash);
    let ses = derive_ses(&transcript_hash);
    let (_, sas) = derive_sas(&transcript_hash);
    let presence = derive_presence_token(&prs, 1_700_000_000);
    // Synthetic 32-byte static_priv — never passed to any logger directly.
    let static_priv = [0x42u8; 32];

    let writer = CapturingWriter::default();
    let buffer_handle = writer.0.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        // Drive several primitives; whether or not they log, none of
        // their inputs/outputs may appear in the captured buffer.
        let _ = derive_prs(&transcript_hash);
        let _ = derive_ses(&transcript_hash);
        let _ = derive_sas(&transcript_hash);
        let _ = derive_presence_token(&prs, 1_700_000_000);

        // Cover the four log levels with non-secret content so the
        // capture writer is exercised end-to-end (test would silently
        // pass if subscriber wiring was broken).
        info!(target: "vortex_test", "info canary");
        warn!(target: "vortex_test", "warn canary");
        debug!(target: "vortex_test", "debug canary");
        error!(target: "vortex_test", "error canary");
    });

    let captured = buffer_handle.lock().unwrap().clone();
    let text = String::from_utf8_lossy(&captured).into_owned();

    // Sanity: the canary lines arrived (subscriber actually wired).
    assert!(text.contains("info canary"), "subscriber not capturing");
    assert!(text.contains("warn canary"));
    assert!(text.contains("debug canary"));
    assert!(text.contains("error canary"));

    // Forbidden hex strings — assert NONE leaked.
    let forbidden: Vec<(&str, String)> = vec![
        ("prs (lower)", hex::encode(prs)),
        ("prs (upper)", hex::encode_upper(prs)),
        ("ses (lower)", hex::encode(ses)),
        ("ses (upper)", hex::encode_upper(ses)),
        ("static_priv (lower)", hex::encode(static_priv)),
        ("presence (lower)", hex::encode(presence)),
        ("sas string", sas.clone()),
    ];
    let mut leaks: Vec<String> = Vec::new();
    for (name, needle) in &forbidden {
        if !needle.is_empty() && text.contains(needle.as_str()) {
            leaks.push(format!("{name} leaked into tracing buffer"));
        }
    }
    assert!(
        leaks.is_empty(),
        "log redaction failed: {leaks:?}\ncaptured:\n{text}",
    );
}

#[test]
fn presence_tokens_rotate_per_bucket() {
    // spec §7.3 — a presence token in bucket T MUST differ from the
    // token in bucket T+1 (and T-1). This is the non-linkability
    // property cited by Phase 9 of the build plan.
    let prs = [0xAAu8; 32];
    let t0 = derive_presence_token(&prs, 1_000);
    let t_prev = derive_presence_token(&prs, 999);
    let t_next = derive_presence_token(&prs, 1_001);
    assert_ne!(t0, t_prev, "presence token did not rotate (prev)");
    assert_ne!(t0, t_next, "presence token did not rotate (next)");
    assert_ne!(t_prev, t_next, "adjacent buckets collide");
    // The token MUST NOT contain the PRS bytes verbatim.
    let pr_hex = hex::encode(prs);
    let t0_hex = hex::encode(t0);
    assert!(!t0_hex.contains(&pr_hex), "presence token leaks PRS bytes");
}

// Silence unused-import lints in the unlikely case rustc decides this
// test file does not exercise a particular module path.
#[allow(dead_code)]
fn _typecheck_module_paths() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
