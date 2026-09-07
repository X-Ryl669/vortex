//! "Open this on my phone" — the mirror of browsing handoff, which until now
//! only ran phone→laptop.
//!
//! You are reading something on the laptop and want it in your hand: send the
//! URL, pick the phone up, tap once. Same request shape as `ring.rs` — a
//! monotonic unix-millis stamp carried in the outgoing AppState, acted on by
//! the phone on the rising edge — because that model has already proven itself
//! against a laptop restart and a lost heartbeat.
//!
//! The phone shows a NOTIFICATION rather than opening the page itself. Android
//! 10 blocks background activity starts, and even if it did not, a phone that
//! opened pages because another device said so is not something to build.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static PAYLOAD: Mutex<Option<String>> = Mutex::new(None);
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Longest thing worth sending this way. A URL or a short snippet is the point;
/// anything larger belongs in the clipboard sync or a file transfer, both of
/// which handle size properly. Also bounds what a heartbeat has to carry.
const MAX_LEN: usize = 2048;

/// Read by the outgoing AppState builders (BLE + LAN).
pub fn pending() -> (Option<String>, u64) {
    let text = PAYLOAD.lock().ok().and_then(|g| g.clone());
    (text, SEQ.load(Ordering::SeqCst))
}

/// Send `text` to the phone. Called by the tray, the `--share` entry point and
/// the UI.
pub fn send(text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("nothing to send".into());
    }
    if text.len() > MAX_LEN {
        return Err(format!("too long ({} bytes, limit {MAX_LEN})", text.len()));
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // Never emit a value <= the last: two sends in the same millisecond, or a
    // clock that stepped back, would otherwise look like a replay and the
    // phone would ignore the second one.
    let next = SEQ
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |prev| Some(now_ms.max(prev + 1)))
        .map(|prev| now_ms.max(prev + 1))
        .unwrap_or(now_ms);
    if let Ok(mut g) = PAYLOAD.lock() {
        *g = Some(text.to_string());
    }
    // Both transports, and now rather than on the next beat: the user is
    // reaching for the phone as they click.
    crate::ble::state_nudge().notify_one();
    if let Some(n) = crate::SYNC_NUDGE.get() {
        n.notify_one();
    }
    tracing::info!(seq = next, len = text.len(), "send-to-phone: queued");
    Ok(())
}

#[tauri::command]
pub(crate) fn send_to_phone(text: String) -> Result<(), String> {
    send(&text)
}
