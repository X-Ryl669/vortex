//! "Ring my phone" (Find-My) — laptop→phone. The user taps Ring on the phone's
//! device card; we bump a monotonic request carried in our outgoing AppState
//! (`ring_seq`). The phone rings loudly, overriding silent/DND, on the rising
//! edge and dedups repeats by the seq. Kept in its own module so `lib.rs`
//! doesn't regrow — mirrors the `camera` request plumbing exactly.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The latest ring request, as unix-millis-at-tap. Millis (not a 1-based
/// counter) so the value only ever increases: a laptop process restart resets
/// this to 0 until the next tap, and the next tap's millis are still greater
/// than any the phone has already seen, so it never regresses into a "replay"
/// the phone would ignore. `0` = never rung this session.
static RING_SEQ: AtomicU64 = AtomicU64::new(0);

/// Read by the laptop's outgoing AppState builder (BLE + LAN) → `ring_seq`.
pub fn ring_seq() -> u64 {
    RING_SEQ.load(Ordering::SeqCst)
}

/// Tauri command: the UI tapped "Ring my phone". Stamp a fresh request and nudge
/// the heartbeat so it ships within ~1s instead of waiting for the next tick.
#[tauri::command]
pub(crate) fn ring_phone() {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // Guard against a clock that didn't advance between two taps (or went
    // backwards): never emit a value <= the last one, or the phone would treat
    // the second tap as a stale repeat and stay silent.
    let next = RING_SEQ
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |prev| {
            Some(now_ms.max(prev + 1))
        })
        .map(|prev| now_ms.max(prev + 1))
        .unwrap_or(now_ms);
    tracing::info!(seq = next, "ring: requested phone ring");
    if let Some(n) = crate::SYNC_NUDGE.get() {
        n.notify_waiters(); // ship ring_seq to the phone now
    }
}
