//! Laptop → phone file push (instant-share style, reverse of the phone→laptop pull).
//! The Tauri UI stages a BATCH of files here (from the Nautilus "Share via
//! Vortex" menu — multiple files, and folders zipped to one archive); the LAN
//! heartbeat's `run_lan_reconnect` takes the batch after the bulk-sync, asks the
//! phone for consent (one prompt per batch), and on accept lets the phone PULL
//! every file through the ranged-filesystem protocol over the same session.
//! One batch at a time; the queue is drained FIFO.
//!
//! # Nothing is held in memory
//!
//! A queued file is a path and a size, not its bytes. On accept, each file is
//! granted a token ([`crate::core::share_grants`]) and the phone reads it in
//! [`crate::core::fs_proto::MAX_READ_LEN`] ranges, writing each one straight to
//! its Downloads. Peak memory is one range on either side whatever the file's
//! size, so there is no size cap to hit — the same reasoning that retired the
//! cap in the phone → laptop direction (`docs/design/file-browsing.md` §1).
//!
//! A phone app that predates the pull answers the offer with
//! [`decision::ACCEPT_STREAM`], and gets the old `FILE_PUSH` chunk stream —
//! read from disk a chunk at a time, so even that path holds nothing on this
//! side.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The phone's answer to a `FILE_PUSH_OFFER` (the single byte of its
/// `FILE_PUSH_DECISION`). Mirrors Kotlin `FilePushDecision`.
pub mod decision {
    /// Declined (or the prompt timed out).
    pub const DECLINE: u8 = 0;
    /// Accepted by a phone app that only knows the legacy chunk stream.
    pub const ACCEPT_STREAM: u8 = 1;
    /// Accepted; the phone will pull every file by its `share:` token and send
    /// a `FILE_PUSH`/[`crate::core::ble::frame::sub::FILE_PUSH_DONE`] report.
    pub const ACCEPT_PULL: u8 = 2;
}

/// Bytes per legacy FILE_PUSH chunk: a big LAN frame (sealed = +4 hdr +16 tag,
/// under the 63 KiB MAX_FRAME_PAYLOAD and the Noise 65535 limit).
pub const PUSH_CHUNK_BYTES: usize = 60 * 1024;

/// A folder we zipped for the share, so the phone can unpack it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderZip {
    /// Sum of the uncompressed sizes of every entry.
    pub unpacked: u64,
    /// Number of file entries.
    pub entries: u64,
}

#[derive(Clone, Debug)]
pub struct OutgoingFile {
    pub name: String,
    pub mime: String,
    /// Where the bytes are read from when the phone pulls.
    pub path: PathBuf,
    /// Size at share time, for the consent prompt and the progress pill. The
    /// pull itself follows the file's real length.
    pub size: u64,
    /// `Some` only for folders WE zipped (`<name>.zip`): tells the phone to
    /// auto-extract into a same-named folder and drop the archive, so a shared
    /// folder lands as a folder. The archive is a temporary file of ours,
    /// deleted when the batch ends ([`discard`]). A user's own `.zip` has this
    /// `None` and is saved as-is — we never silently unpack their archive.
    ///
    /// The declared totals are what the phone holds the archive to while
    /// unpacking: the user consented to this much, so an archive that expands
    /// past it is not the one they agreed to.
    pub folder: Option<FolderZip>,
}

/// One user "Share" action — one or more files (folders already zipped to a
/// single archive entry by the caller), with when it was queued. Sent under a
/// single consent prompt.
static QUEUE: Mutex<VecDeque<(Instant, Vec<OutgoingFile>)>> = Mutex::new(VecDeque::new());

/// Stage a batch for the next heartbeat to push. Ignored if empty.
pub fn enqueue_batch(files: Vec<OutgoingFile>) -> bool {
    if files.is_empty() {
        return false;
    }
    if let Ok(mut q) = QUEUE.lock() {
        q.push_back((Instant::now(), files));
        true
    } else {
        false
    }
}

/// Take the next queued batch (FIFO), if any.
pub fn take_batch() -> Option<Vec<OutgoingFile>> {
    QUEUE.lock().ok().and_then(|mut q| q.pop_front()).map(|(_, f)| f)
}

/// Whether anything is waiting (so the heartbeat round knows to nudge again).
pub fn pending() -> bool {
    QUEUE.lock().map(|q| !q.is_empty()).unwrap_or(false)
}

/// When the oldest batch still waiting was queued. A batch leaves the queue
/// the moment a session takes it, so a long wait here means no session has
/// reached the phone since — which is what the Wi-Fi Direct watchdog needs to
/// tell a stuck link from a slow transfer.
pub fn pending_since() -> Option<Instant> {
    QUEUE.lock().ok().and_then(|q| q.front().map(|(t, _)| *t))
}

/// A batch taken off the queue and not finished yet.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Whether a batch is being offered or pulled right now. It has left the
/// queue, so [`pending`] no longer sees it, but the link is busy with it.
pub fn in_flight() -> bool {
    IN_FLIGHT.load(Ordering::Relaxed)
}

/// Marks a batch as in flight for as long as it lives; dropped however the
/// push ends, early returns and errors included.
pub struct InFlight(());

impl InFlight {
    pub fn begin() -> Self {
        IN_FLIGHT.store(true, Ordering::Relaxed);
        InFlight(())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        IN_FLIGHT.store(false, Ordering::Relaxed);
    }
}

/// How a batch is named in the pill and the phone's prompt: the file's name
/// for one file, a count otherwise.
pub fn batch_label(files: &[OutgoingFile]) -> String {
    match files {
        [one] => one.name.clone(),
        _ => format!("{} files", files.len()),
    }
}

/// Where folder archives are staged while they wait to be pulled.
pub fn temp_dir() -> Option<PathBuf> {
    crate::core::platform::paths()
        .cache()
        .map(|c| c.join("outgoing"))
}

/// Delete the temporary archives of a finished batch. The user's own files are
/// never touched — only the archives we made from their folders.
pub fn discard(files: &[OutgoingFile]) {
    for f in files.iter().filter(|f| f.folder.is_some()) {
        if let Err(e) = std::fs::remove_file(&f.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(name = %f.name, "share: couldn't remove temp archive: {e}");
            }
        }
    }
}

/// File name for a new staging archive. Prefixed with our PID so
/// [`purge_temp_dir`] can tell an orphan from an archive in flight.
pub fn temp_name(unique: &str) -> String {
    format!("{}-{unique}.zip", std::process::id())
}

/// Delete archives left by earlier processes. The queue lives in memory, so
/// those can never be sent. Archives of THIS process are kept: a share
/// forwarded while the app is starting may already be staged.
pub fn purge_temp_dir() {
    let Some(dir) = temp_dir() else { return };
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let ours = format!("{}-", std::process::id());
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().starts_with(&ours) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// What the phone reported once it finished pulling a batch.
#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct PushReport {
    /// Files saved to its Downloads.
    #[serde(default)]
    pub saved: u32,
    /// Names of the files it could not save.
    #[serde(default)]
    pub failed: Vec<String>,
}

/// Progress of an outgoing batch, reported by `push_outgoing_batch` so the Tauri
/// UI can drive a "Sending …" top-bar pill (mirror of `file_progress` for the
/// receive direction). No-op until the UI installs a hook.
pub enum OutProgress {
    /// Batch staged & offer about to go out — pill shows "Waiting for phone…".
    Start { label: String, count: u32, total: u64 },
    /// Phone accepted — pill flips to "Sending…".
    Accepted,
    /// Phone declined (or timed out) — terminal.
    Declined,
    /// Cumulative bytes streamed across the whole batch.
    Progress { sent: u64, total: u64 },
    /// The batch is over — terminal. `failed` names the files the phone could
    /// not save; empty means everything arrived.
    Done { failed: Vec<String> },
    /// The transfer broke off — terminal. `reason` is short and human-readable.
    Fail { reason: String },
}

type ProgHook = Box<dyn Fn(OutProgress) + Send + Sync>;
static PROG_HOOK: OnceLock<ProgHook> = OnceLock::new();

/// Install the send-progress sink (the UI's forwarder). Idempotent — first wins.
pub fn set_progress_hook(hook: ProgHook) {
    let _ = PROG_HOOK.set(hook);
}

/// Report a step of the current outgoing batch. Cheap; safe to call per chunk.
pub fn report_progress(ev: OutProgress) {
    if let Some(h) = PROG_HOOK.get() {
        h(ev);
    }
}

/// How many legacy chunks a file of `size` bytes needs, or `None` when the
/// legacy wire cannot carry it: its `[total u16][idx u16]` header counts at
/// most `u16::MAX` chunks. Only the legacy stream has this limit — a pull has
/// none. An empty file still takes one (empty) chunk, so the phone's assembler
/// gets a header to complete on.
pub fn legacy_chunk_count(size: u64) -> Option<u16> {
    let n = size.div_ceil(PUSH_CHUNK_BYTES as u64).max(1);
    u16::try_from(n).ok()
}

/// One legacy chunk payload: `[total u16][idx u16][data]`, reassembled by the
/// phone's `ClipboardImageAssembler`.
pub fn legacy_chunk(total: u16, idx: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&idx.to_be_bytes());
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_chunk_count_covers_empty_exact_and_oversized() {
        // An empty file still sends one header-only chunk.
        assert_eq!(legacy_chunk_count(0), Some(1));
        assert_eq!(legacy_chunk_count(1), Some(1));
        // Exact multiple: no phantom tail chunk.
        assert_eq!(legacy_chunk_count(2 * PUSH_CHUNK_BYTES as u64), Some(2));
        assert_eq!(legacy_chunk_count(2 * PUSH_CHUNK_BYTES as u64 + 1), Some(3));
        // Past what a u16 chunk counter can address.
        let max = u16::MAX as u64 * PUSH_CHUNK_BYTES as u64;
        assert_eq!(legacy_chunk_count(max), Some(u16::MAX));
        assert_eq!(legacy_chunk_count(max + 1), None);
    }

    #[test]
    fn legacy_chunk_header_is_big_endian_total_then_index() {
        let c = legacy_chunk(3, 1, b"xy");
        assert_eq!(c, vec![0, 3, 0, 1, b'x', b'y']);
    }

    /// Single test owns the global QUEUE so the assertions stay sequential
    /// (other test modules never touch it). Drains first for a clean slate.
    #[test]
    fn queue_is_fifo() {
        while take_batch().is_some() {}

        let f = |n: &str| OutgoingFile {
            name: n.into(),
            mime: "application/octet-stream".into(),
            path: PathBuf::from(format!("/nonexistent/{n}")),
            size: 8,
            folder: None,
        };

        // Empty batch rejected, queue untouched.
        assert!(!enqueue_batch(vec![]));
        assert!(!pending());

        // Two batches in → FIFO out.
        assert!(enqueue_batch(vec![f("a"), f("b")]));
        assert!(enqueue_batch(vec![f("c")]));
        assert!(pending());

        let first = take_batch().expect("first batch");
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].name, "a");
        let second = take_batch().expect("second batch");
        assert_eq!(second[0].name, "c");

        assert!(take_batch().is_none(), "drained");
        assert!(!pending());
        assert!(pending_since().is_none());

        // The oldest batch dates the wait.
        let before = Instant::now();
        assert!(enqueue_batch(vec![f("d")]));
        assert!(pending_since().expect("waiting") >= before);
        assert_eq!(batch_label(&take_batch().expect("d")), "d");
        assert_eq!(batch_label(&[f("x"), f("y")]), "2 files");
    }

    #[test]
    fn push_report_tolerates_missing_fields() {
        let r: PushReport = serde_json::from_str("{}").expect("parses");
        assert_eq!(r.saved, 0);
        assert!(r.failed.is_empty());
        let r: PushReport =
            serde_json::from_str(r#"{"saved":2,"failed":["a.pdf"]}"#).expect("parses");
        assert_eq!((r.saved, r.failed), (2, vec!["a.pdf".to_string()]));
    }

    #[test]
    fn discard_removes_only_our_archives() {
        let dir = std::env::temp_dir().join("vortex-outgoing-discard");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let user = dir.join("user.zip");
        let ours = dir.join("folder.zip");
        std::fs::write(&user, b"u").expect("write");
        std::fs::write(&ours, b"o").expect("write");
        discard(&[
            OutgoingFile {
                name: "user.zip".into(),
                mime: "application/zip".into(),
                path: user.clone(),
                size: 1,
                folder: None,
            },
            OutgoingFile {
                name: "folder.zip".into(),
                mime: "application/zip".into(),
                path: ours.clone(),
                size: 1,
                folder: Some(FolderZip {
                    unpacked: 1,
                    entries: 1,
                }),
            },
        ]);
        assert!(user.exists(), "the user's own file is never deleted");
        assert!(!ours.exists(), "our temp archive is");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
