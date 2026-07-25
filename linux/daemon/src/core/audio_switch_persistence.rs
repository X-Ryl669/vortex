//! Crash-recovery persistence for `audio_orchestrator::SwitchOrchestrator`
//! (R1 in `the earbuds-switch design notes` §3).
//!
//! Stored as a single JSON object at `$XDG_STATE_HOME/vortex/audio_switch.json`
//! (falling back to `~/.local/state/vortex/`). Plain JSON — not
//! Secret Service — because:
//!
//! * The contents are short-lived runtime state, not credentials.
//! * Inspectability matters more than secrecy for debugging stuck flows.
//! * The pairing PRS and identity keys (the things that genuinely
//!   require encryption) live in Secret Service already.
//!
//! On every state transition the orchestrator overwrites this file
//! atomically (write-to-temp + rename). On daemon start, [`recover`]
//! reads the file and decides whether to resume an in-flight Connecting
//! attempt or roll back. Resume policy mirrors the Android side:
//!
//! | persisted          | age ≤ 10 s  | age 10–30 s   | age > 30 s          |
//! | ------------------ | ----------- | ------------- | ------------------- |
//! | `connecting`       | resume      | rollback      | rollback (stale)    |
//! | `waiting_*`        | rollback (peer's view moved on)              |
//! | `failed`           | rollback, surface previous reason            |
//! | `idle` / absent    | no-op                                        |
//!
//! Resume is safe: the BT connect call is idempotent — if the buds
//! came back automatically while the daemon was down, BlueZ reports
//! "already connected" and we emit `Done` straight away.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Resume window — `Connecting` newer than this gets retried.
pub const RESUME_CONNECT_MAX_MS: u64 = 10_000;

/// Above this age, switch context is considered meaningless and we
/// roll back unconditionally regardless of which state was stored.
pub const STALE_TTL_MS: u64 = 30_000;

/// Stable lowercase discriminators. Renaming a state in source must
/// not break decode — these strings are the wire contract for the
/// persistence blob.
pub mod disc {
    pub const IDLE: &str = "idle";
    pub const PREPARING: &str = "preparing";
    pub const WAITING_APPROVAL: &str = "waiting_approval";
    pub const WAITING_RELEASED: &str = "waiting_released";
    pub const CONNECTING: &str = "connecting";
    pub const FAILED: &str = "failed";
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Saved {
    pub discriminator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Unix milliseconds; used to compute age.
    pub enter_ms: u64,
    /// Hex-encoded peer static public key. Empty string when no peer
    /// context applies (e.g. a degenerate `Failed` written before any
    /// flow started). Empty decodes to `None` peer.
    #[serde(default)]
    pub peer_pub_hex: String,
    #[serde(default)]
    pub mac: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Rollback(String),
    ResumeConnect { peer_pub: [u8; 32], mac: String },
}

/// State directory path. Honours `XDG_STATE_HOME`, falls back to
/// `~/.local/state`. Returns `None` only if neither env var is set —
/// in that case persistence is silently disabled, which is the
/// correct behaviour for an oddly-configured environment (the
/// orchestrator still runs; we just don't survive crashes).
fn state_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))?;
    Some(base.join("vortex").join("audio_switch.json"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Write the saved blob to disk. We write to `<path>.tmp` first then
/// atomically rename — this prevents a torn read if we crash between
/// open() and the closing fsync. Best-effort: any I/O error is
/// returned so the caller can log it, but a failure to persist does
/// not abort the in-progress switch.
pub fn save(saved: &Saved) -> std::io::Result<()> {
    let Some(path) = state_path() else {
        return Ok(());
    };
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(saved)
        .map_err(std::io::Error::other)?;
    crate::core::fs_private::write_private(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)
}

pub fn clear() -> std::io::Result<()> {
    let Some(path) = state_path() else {
        return Ok(());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn load() -> Option<Saved> {
    let path = state_path()?;
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Apply the recovery decision tree to a persisted blob. Pure
/// function of `(saved, now)` — easy to unit-test without touching
/// the filesystem.
pub fn decide(saved: Option<&Saved>, now_ms_val: u64) -> Action {
    let Some(s) = saved else { return Action::None };
    if s.discriminator == disc::IDLE {
        return Action::None;
    }
    let age = now_ms_val.saturating_sub(s.enter_ms);
    if age > STALE_TTL_MS {
        return Action::Rollback(format!("previous switch stale ({}s); rolled back", age / 1000));
    }
    match s.discriminator.as_str() {
        disc::CONNECTING => {
            if age <= RESUME_CONNECT_MAX_MS {
                if let Some(peer) = hex_to_pub(&s.peer_pub_hex) {
                    if !s.mac.is_empty() {
                        return Action::ResumeConnect { peer_pub: peer, mac: s.mac.clone() };
                    }
                }
            }
            Action::Rollback("previous Connecting too old or missing context".to_string())
        }
        disc::FAILED => Action::Rollback(
            s.reason.clone().unwrap_or_else(|| "previous switch failed".to_string()),
        ),
        other => Action::Rollback(format!("previous switch interrupted ({other})")),
    }
}

/// Convenience: load + decide using the wall clock. Returns
/// [`Action::None`] if nothing was persisted or the file was Idle.
pub fn recover() -> Action {
    decide(load().as_ref(), now_ms())
}

fn hex_to_pub(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        let byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
        out[i] = byte;
    }
    Some(out)
}

pub fn pub_to_hex(p: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in p {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer() -> [u8; 32] {
        let mut p = [0u8; 32];
        for i in 0..32 {
            p[i] = (i as u8).wrapping_mul(7);
        }
        p
    }

    fn saved(disc_str: &str, age_ms: u64, with_ctx: bool) -> Saved {
        Saved {
            discriminator: disc_str.to_string(),
            reason: None,
            enter_ms: 100_000 - age_ms.min(100_000),
            peer_pub_hex: if with_ctx { pub_to_hex(&peer()) } else { String::new() },
            mac: if with_ctx { "AC:47:1B:25:71:C2".to_string() } else { String::new() },
        }
    }

    #[test]
    fn none_when_no_blob() {
        assert_eq!(decide(None, 100_000), Action::None);
    }

    #[test]
    fn idle_means_none() {
        let s = saved(disc::IDLE, 0, true);
        assert_eq!(decide(Some(&s), 100_000), Action::None);
    }

    #[test]
    fn connecting_fresh_resumes() {
        let s = saved(disc::CONNECTING, 5_000, true);
        let a = decide(Some(&s), 100_000);
        match a {
            Action::ResumeConnect { peer_pub, mac } => {
                assert_eq!(peer_pub, peer());
                assert_eq!(mac, "AC:47:1B:25:71:C2");
            }
            other => panic!("expected ResumeConnect, got {other:?}"),
        }
    }

    #[test]
    fn connecting_15s_old_rolls_back() {
        let s = saved(disc::CONNECTING, 15_000, true);
        match decide(Some(&s), 100_000) {
            Action::Rollback(r) => assert!(r.contains("too old"), "{r}"),
            other => panic!("expected Rollback, got {other:?}"),
        }
    }

    #[test]
    fn connecting_missing_ctx_rolls_back() {
        let s = saved(disc::CONNECTING, 5_000, false);
        assert!(matches!(decide(Some(&s), 100_000), Action::Rollback(_)));
    }

    #[test]
    fn waiting_approval_rolls_back_with_interrupted_reason() {
        let s = saved(disc::WAITING_APPROVAL, 1_000, true);
        match decide(Some(&s), 100_000) {
            Action::Rollback(r) => assert!(r.contains("interrupted"), "{r}"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn failed_surfaces_reason() {
        let mut s = saved(disc::FAILED, 1_000, true);
        s.reason = Some("BT off".to_string());
        match decide(Some(&s), 100_000) {
            Action::Rollback(r) => assert_eq!(r, "BT off"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn beyond_stale_ttl_always_rolls_back() {
        let s = Saved {
            discriminator: disc::CONNECTING.to_string(),
            reason: None,
            enter_ms: 0,
            peer_pub_hex: pub_to_hex(&peer()),
            mac: "AC:47:1B:25:71:C2".to_string(),
        };
        // 60s elapsed; STALE_TTL_MS = 30_000.
        match decide(Some(&s), 60_000) {
            Action::Rollback(r) => assert!(r.contains("stale"), "{r}"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn hex_round_trip() {
        let p = peer();
        let h = pub_to_hex(&p);
        assert_eq!(h.len(), 64);
        assert_eq!(hex_to_pub(&h).unwrap(), p);
    }

    #[test]
    fn hex_rejects_wrong_length() {
        assert!(hex_to_pub("abcd").is_none());
        assert!(hex_to_pub(&"f".repeat(63)).is_none());
        assert!(hex_to_pub(&"f".repeat(65)).is_none());
    }

    #[test]
    fn hex_rejects_invalid_chars() {
        let mut h = pub_to_hex(&peer());
        h.replace_range(0..1, "z");
        assert!(hex_to_pub(&h).is_none());
    }
}
