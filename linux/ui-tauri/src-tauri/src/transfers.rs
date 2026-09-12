//! Ongoing file-transfer indicator, shown as a LIVE ACTIVITY — the same
//! top-bar "pill" the in-call timer uses (drawn by the Vortex GNOME Shell
//! extension if installed, else a per-activity tray icon + progress menu). One
//! pill aggregates the current batch of phone→laptop file shares: it updates in
//! place as bytes arrive (no toast, no re-sliding banner) and resolves to
//! "Received N → Downloads" before disappearing. Session-only.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use tokio::sync::mpsc::UnboundedSender;
use vortex_l3_daemon::core::live_activity::LiveActivity;

/// Stable pill key — one transfer pill at a time (the batch).
const PILL_KEY: &str = "vortex-file-transfer";

struct Item {
    id: u64,
    name: String,
    size: u64,
    received: u64,
    done: bool,
    failed: bool,
    /// The subfolder a capture lands in (see `Offer::subdir`); `None` for a
    /// share, which goes to the download folder's root.
    subdir: Option<&'static str>,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static LIVE_TX: OnceLock<UnboundedSender<LiveActivity>> = OnceLock::new();

/// Receive the live-activity channel (the same one the call pill / phone live
/// activities use), wired at worker start.
pub(crate) fn init(live_tx: UnboundedSender<LiveActivity>) {
    // Make the Vortex logo available at the "vortex" app_id path for the pill.
    let _ = vortex_l3_daemon::core::icon_cache::ensure_vortex();
    let _ = LIVE_TX.set(live_tx);
}

pub(crate) fn start(name: &str, size: u64, subdir: Option<&'static str>) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut g) = ITEMS.lock() {
        g.push(Item {
            id,
            name: name.to_string(),
            size,
            received: 0,
            done: false,
            failed: false,
            subdir,
        });
    }
    emit();
    id
}

pub(crate) fn set_progress_chunks(id: u64, received_chunks: u32, total_chunks: u32) {
    if let Ok(mut g) = ITEMS.lock() {
        if let Some(it) = g.iter_mut().find(|i| i.id == id) {
            if !it.done {
                it.received = if total_chunks > 0 {
                    (received_chunks as u64).saturating_mul(it.size) / total_chunks as u64
                } else {
                    0
                };
            }
        }
    }
    emit();
}

pub(crate) fn complete(id: u64) {
    if let Ok(mut g) = ITEMS.lock() {
        if let Some(it) = g.iter_mut().find(|i| i.id == id) {
            it.done = true;
            it.received = it.size;
        }
    }
    emit();
}

pub(crate) fn fail(id: u64) {
    if let Ok(mut g) = ITEMS.lock() {
        if let Some(it) = g.iter_mut().find(|i| i.id == id) {
            it.failed = true;
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

/// The pill's own action verbs, arriving from the GNOME extension's buttons
/// on the same channel the call pill uses. Returns true when the verb was
/// ours, so the caller knows not to pass it on to the phone.
pub(crate) fn handle_pill_action(verb: &str) -> bool {
    match verb {
        "xfer:cancel" => {
            cancel_all();
            true
        }
        _ => false,
    }
}

/// Stop the batch: drop whatever is still queued, cut the pull in flight, and
/// let the pill resolve as failed.
///
/// Everything, not one file: the pill shows the batch as one thing, so its one
/// button has to mean what the thing it sits on says. A partly-written file is
/// left to the receive path, which already writes to a temporary name and only
/// renames a whole file into place — so a cancelled pull leaves nothing behind
/// that looks complete.
fn cancel_all() {
    let dropped = crate::PENDING_FILE_OFFERS
        .get()
        .and_then(|q| q.lock().ok().map(|mut g| g.drain(..).count()))
        .unwrap_or(0);
    vortex_l3_daemon::core::file_progress::request_cancel();
    let mut cancelled = 0usize;
    if let Ok(mut g) = ITEMS.lock() {
        for it in g.iter_mut().filter(|i| !i.done && !i.failed) {
            it.failed = true;
            cancelled += 1;
        }
    }
    tracing::info!(queued = dropped, in_flight = cancelled, "file transfer cancelled by the user");
    emit();
}

/// Where the last finished batch saved, as a `file://` URI for the pill's
/// click. Kept across the batch reset because the pill outlives it by a few
/// seconds — which is exactly the window someone looks up and wants the file.
static LAST_DIR: Mutex<Option<String>> = Mutex::new(None);

/// Note where a received file landed, for the finished pill to open.
pub(crate) fn note_saved(path: &std::path::Path) {
    let Some(dir) = path.parent() else { return };
    if let Ok(mut g) = LAST_DIR.lock() {
        *g = Some(format!("file://{}", dir.display()));
    }
}

fn pill(title: String, text: String, progress: i32, ended: bool, open: String) -> LiveActivity {
    LiveActivity {
        key: PILL_KEY.to_string(),
        app: "Vortex".to_string(),
        // Resolves to cache/vortex.png (ensured in init) → the Vortex logo,
        // not the generic bell.
        app_id: "vortex".to_string(),
        title,
        text,
        sub: String::new(),
        open,
        progress,
        started_at: 0,
        muted: false,
        speaker: false,
        has_earbuds: false,
        ended,
        playing: None,
    }
}

/// Aggregate the batch and push the pill; resolve + schedule its removal once
/// everything finishes.
fn emit() {
    let (title, text, progress, all_done) = {
        let g = match ITEMS.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if g.is_empty() {
            return;
        }
        let count = g.len();
        let done_n = g.iter().filter(|i| i.done || i.failed).count();
        let total: u64 = g.iter().map(|i| i.size).sum();
        let received: u64 = g
            .iter()
            .map(|i| if i.done { i.size } else { i.received })
            .sum();
        let all_done = g.iter().all(|i| i.done || i.failed);
        let pct = if total > 0 {
            (received.saturating_mul(100) / total) as i32
        } else {
            0
        };
        // One aggregate pill for the whole batch (instant-share style: a single
        // "Receiving N items", not one per file). Multi-file gets an "X/N"
        // completion count so each finishing file is still visible.
        let label = if count == 1 {
            g[0].name.clone()
        } else {
            format!("{count} files")
        };
        if all_done {
            // Name the subfolder only when the whole batch went to one; a
            // mixed batch (a share debounced in beside a screenshot) is
            // described by the root they all sit under.
            let subdir = g[0].subdir.filter(|_| g.iter().all(|i| i.subdir == g[0].subdir));
            (
                format!("Received {label}"),
                // Name the folder they actually landed in — it's localised
                // ("Téléchargements", …), and a wrong name here sends the user
                // hunting in a folder that hasn't got the files.
                format!("Saved to {}", crate::clipboard_sync::receive_label(subdir)),
                100,
                true,
            )
        } else if count == 1 {
            (
                format!("Receiving {label}"),
                format!("{} / {}", fmt_bytes(received), fmt_bytes(total)),
                pct,
                false,
            )
        } else {
            (
                format!("Receiving {count} files"),
                format!(
                    "{done_n}/{count} · {} / {}",
                    fmt_bytes(received),
                    fmt_bytes(total)
                ),
                pct,
                false,
            )
        }
    };

    // Only the finished pill is worth clicking; mid-transfer there is nothing
    // in the folder yet to go and look at.
    let open = if all_done {
        LAST_DIR.lock().ok().and_then(|g| g.clone()).unwrap_or_default()
    } else {
        String::new()
    };
    send(pill(title, text, progress, false, open));

    if all_done {
        // Reset the batch so the next share starts fresh, then remove the pill
        // shortly after — unless a NEW batch has begun in the meantime.
        if let Ok(mut g) = ITEMS.lock() {
            g.clear();
        }
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let idle = ITEMS.lock().map(|g| g.is_empty()).unwrap_or(true);
            if idle {
                send(pill(String::new(), String::new(), -1, true, String::new()));
            }
        });
    }
}
