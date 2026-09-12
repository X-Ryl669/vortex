//! Tiny decoupling hook so the LAN file-pull (in `core::lan::tcp_client`, this
//! crate) can report incoming-FILE progress to the Tauri UI (the binary crate)
//! WITHOUT a dependency edge. The UI installs a callback once at startup; the
//! pull calls [`report`] per chunk. No-op until a hook is set.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

type Hook = Box<dyn Fn(u32, u32) + Send + Sync>;

static HOOK: OnceLock<Hook> = OnceLock::new();

/// Install the progress sink (the UI's forwarder). Idempotent — first wins.
pub fn set_hook(hook: Hook) {
    let _ = HOOK.set(hook);
}

/// Report progress of the file currently being pulled: `received_chunks` of
/// `total_chunks`. Cheap and lossy — fine to call on every chunk.
pub fn report(received_chunks: u32, total_chunks: u32) {
    if let Some(h) = HOOK.get() {
        h(received_chunks, total_chunks);
    }
}

/// Set when the user asks to stop the transfer in flight.
///
/// A pull is a long run of chunks inside one bulk-sync round, and until now
/// nothing could interrupt it: a file picked by mistake had to finish. The
/// chunk loop checks this between frames — the only place it can, and often
/// enough that a cancel lands within one chunk of the click.
static CANCEL: AtomicBool = AtomicBool::new(false);

/// Ask the pull in flight to stop. Cleared by [`take_cancel`] when it does.
pub fn request_cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// True once, if a cancel is pending — and clears it, so the flag cannot leak
/// into the next transfer. The next round starts with a clean slate whatever
/// happened to this one.
pub fn take_cancel() -> bool {
    CANCEL.swap(false, Ordering::Relaxed)
}
