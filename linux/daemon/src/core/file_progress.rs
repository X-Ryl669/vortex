//! Tiny decoupling hook so the LAN file-pull (in `core::lan::tcp_client`, this
//! crate) can report incoming-FILE progress to the Tauri UI (the binary crate)
//! WITHOUT a dependency edge. The UI installs a callback once at startup; the
//! pull calls [`report`] per chunk. No-op until a hook is set.

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
