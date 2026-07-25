//! Outgoing file-transfer indicator (laptop → phone), shown as the same top-bar
//! "pill" live activity the receive side uses (`transfers.rs`) — worded for
//! sending: "Waiting for phone…" → "Sending <label>" → "Sent <label>" (or
//! "Declined by phone"). Driven by the daemon's `outgoing_share` progress hook.
//! One batch at a time (the push queue is FIFO); a batch aggregates all its
//! files into a single pill. Session-only.

use std::sync::Mutex;
use std::sync::OnceLock;

use tokio::sync::mpsc::UnboundedSender;
use vortex_l3_daemon::core::live_activity::LiveActivity;

/// Stable pill key — distinct from the receive pill so a simultaneous send and
/// receive each get their own top-bar slot.
const PILL_KEY: &str = "vortex-file-send";

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Waiting,
    Sending,
    Done,
    Declined,
    Failed,
}

struct Batch {
    label: String,
    total: u64,
    sent: u64,
    phase: Phase,
}

static BATCH: Mutex<Option<Batch>> = Mutex::new(None);
static LIVE_TX: OnceLock<UnboundedSender<LiveActivity>> = OnceLock::new();

/// Receive the live-activity channel (shared with the call / receive pills).
pub(crate) fn init(live_tx: UnboundedSender<LiveActivity>) {
    let _ = vortex_l3_daemon::core::icon_cache::ensure_vortex();
    let _ = LIVE_TX.set(live_tx);
}

pub(crate) fn start(label: &str, total: u64) {
    if let Ok(mut g) = BATCH.lock() {
        *g = Some(Batch {
            label: label.to_string(),
            total,
            sent: 0,
            phase: Phase::Waiting,
        });
    }
    emit();
}

pub(crate) fn accepted() {
    set_phase(Phase::Sending);
}

pub(crate) fn declined() {
    set_phase(Phase::Declined);
}

pub(crate) fn set_progress(sent: u64, total: u64) {
    if let Ok(mut g) = BATCH.lock() {
        if let Some(b) = g.as_mut() {
            if b.phase != Phase::Done && b.phase != Phase::Declined && b.phase != Phase::Failed {
                b.phase = Phase::Sending;
                b.sent = sent;
                if total > 0 {
                    b.total = total;
                }
            }
        }
    }
    emit();
}

pub(crate) fn complete() {
    if let Ok(mut g) = BATCH.lock() {
        if let Some(b) = g.as_mut() {
            b.phase = Phase::Done;
            b.sent = b.total;
        }
    }
    emit();
}

pub(crate) fn fail() {
    set_phase(Phase::Failed);
}

fn set_phase(p: Phase) {
    if let Ok(mut g) = BATCH.lock() {
        if let Some(b) = g.as_mut() {
            b.phase = p;
        }
    }
    emit();
}

fn fmt_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{} KB", n / 1024)
    } else if n < 1024 * 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    } else {
        format!("{:.2} GB", n as f64 / 1024.0 / 1024.0 / 1024.0)
    }
}

fn send(la: LiveActivity) {
    if let Some(tx) = LIVE_TX.get() {
        let _ = tx.send(la);
    }
}

fn pill(title: String, text: String, progress: i32, ended: bool) -> LiveActivity {
    LiveActivity {
        key: PILL_KEY.to_string(),
        app: "Vortex".to_string(),
        app_id: "vortex".to_string(),
        title,
        text,
        sub: String::new(),
        progress,
        started_at: 0,
        muted: false,
        speaker: false,
        has_earbuds: false,
        ended,
        playing: None,
    }
}

fn emit() {
    let (title, text, progress, terminal) = {
        let g = match BATCH.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let b = match g.as_ref() {
            Some(b) => b,
            None => return,
        };
        let pct = if b.total > 0 {
            (b.sent.saturating_mul(100) / b.total) as i32
        } else {
            0
        };
        match b.phase {
            Phase::Waiting => (
                format!("Sharing {}", b.label),
                "Waiting for phone…".to_string(),
                -1,
                false,
            ),
            Phase::Sending => (
                format!("Sending {}", b.label),
                format!("{} / {}", fmt_bytes(b.sent), fmt_bytes(b.total)),
                pct,
                false,
            ),
            Phase::Done => (
                format!("Sent {}", b.label),
                "Delivered to phone".to_string(),
                100,
                true,
            ),
            Phase::Declined => (
                format!("Declined: {}", b.label),
                "Phone declined".to_string(),
                0,
                true,
            ),
            Phase::Failed => (
                format!("Send failed: {}", b.label),
                String::new(),
                0,
                true,
            ),
        }
    };

    send(pill(title, text, progress, false));

    if terminal {
        if let Ok(mut g) = BATCH.lock() {
            *g = None;
        }
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(4));
            let idle = BATCH.lock().map(|g| g.is_none()).unwrap_or(true);
            if idle {
                send(pill(String::new(), String::new(), -1, true));
            }
        });
    }
}
