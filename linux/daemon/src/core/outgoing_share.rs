//! Laptop → phone file push (instant-share style, reverse of the phone→laptop pull).
//! The Tauri UI stages a BATCH of files here (from the Nautilus "Share via
//! Vortex" menu — multiple files, and folders zipped to one archive); the LAN
//! heartbeat's `run_lan_reconnect` takes the batch after the bulk-sync, asks the
//! phone for consent (one prompt per batch), and on accept streams every file
//! over the same session (so the phone's control loop reads it in nonce
//! lock-step). One batch at a time; the queue is drained FIFO.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// Hard cap per file — bounds the in-memory staging buffer.
pub const MAX_PUSH_BYTES: usize = 64 * 1024 * 1024;

/// Hard cap on a whole batch (all files held in memory at once).
pub const MAX_BATCH_BYTES: usize = 256 * 1024 * 1024;

/// Bytes per FILE_PUSH chunk: a big LAN frame (sealed = +4 hdr +16 tag, under
/// the 63 KiB MAX_FRAME_PAYLOAD and the Noise 65535 limit).
pub const PUSH_CHUNK_BYTES: usize = 60 * 1024;

#[derive(Clone)]
pub struct OutgoingFile {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    /// True only for folders WE zipped (`<name>.zip`): tells the phone to
    /// auto-extract into a same-named folder and drop the archive, so a shared
    /// folder lands as a folder (a step beyond plain file share). A user's own
    /// `.zip` file has this false and is saved as-is — we never silently unpack
    /// their archive.
    pub extract: bool,
}

/// One user "Share" action — one or more files (folders already zipped to a
/// single archive entry by the caller). Sent under a single consent prompt.
static QUEUE: Mutex<VecDeque<Vec<OutgoingFile>>> = Mutex::new(VecDeque::new());

/// Stage a batch for the next heartbeat to push. Ignored if empty or over cap.
pub fn enqueue_batch(files: Vec<OutgoingFile>) -> bool {
    if files.is_empty() {
        return false;
    }
    let total: usize = files.iter().map(|f| f.bytes.len()).sum();
    if total > MAX_BATCH_BYTES {
        return false;
    }
    if let Ok(mut q) = QUEUE.lock() {
        q.push_back(files);
        true
    } else {
        false
    }
}

/// Take the next queued batch (FIFO), if any.
pub fn take_batch() -> Option<Vec<OutgoingFile>> {
    QUEUE.lock().ok().and_then(|mut q| q.pop_front())
}

/// Whether anything is waiting (so the heartbeat round knows to nudge again).
pub fn pending() -> bool {
    QUEUE.lock().map(|q| !q.is_empty()).unwrap_or(false)
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
    /// All files delivered — terminal.
    Done,
    /// Transport error mid-batch — terminal.
    Fail,
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

/// Split bytes into `[total u16][idx u16][data]` chunks (same wire as the
/// image/file pull), reassembled by the phone's `ClipboardImageAssembler`.
pub fn build_chunks(bytes: &[u8]) -> Vec<Vec<u8>> {
    let total = bytes.len().div_ceil(PUSH_CHUNK_BYTES).max(1) as u16;
    // `[u8]::chunks` yields nothing for an empty slice; emit a single 0/1 frame
    // so an empty file still delivers a header (the phone's assembler keys total
    // off the first chunk — zero chunks would dangle the offer forever).
    if bytes.is_empty() {
        return vec![{
            let mut out = Vec::with_capacity(4);
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out
        }];
    }
    bytes
        .chunks(PUSH_CHUNK_BYTES)
        .enumerate()
        .map(|(idx, chunk)| {
            let mut out = Vec::with_capacity(4 + chunk.len());
            out.extend_from_slice(&total.to_be_bytes());
            out.extend_from_slice(&(idx as u16).to_be_bytes());
            out.extend_from_slice(chunk);
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse one `[total u16][idx u16][data]` chunk back to its parts — mirrors
    /// what the phone's `ClipboardImageAssembler` reads off the wire.
    fn parse(c: &[u8]) -> (u16, u16, Vec<u8>) {
        (
            u16::from_be_bytes([c[0], c[1]]),
            u16::from_be_bytes([c[2], c[3]]),
            c[4..].to_vec(),
        )
    }

    #[test]
    fn build_chunks_roundtrip_and_header() {
        // 2 full chunks + a short tail ⇒ 3 chunks.
        let bytes: Vec<u8> = (0..(2 * PUSH_CHUNK_BYTES + 11))
            .map(|i| (i % 251) as u8)
            .collect();
        let chunks = build_chunks(&bytes);
        assert_eq!(chunks.len(), 3);

        let mut reassembled = Vec::new();
        for (idx, c) in chunks.iter().enumerate() {
            let (total, i, data) = parse(c);
            assert_eq!(total, 3, "every chunk carries the true total");
            assert_eq!(i as usize, idx, "idx is monotonic from 0");
            reassembled.extend_from_slice(&data);
        }
        assert_eq!(reassembled, bytes, "byte-exact reassembly");
    }

    #[test]
    fn build_chunks_empty_yields_one_empty_chunk() {
        // div_ceil(0)=0 but `.max(1)` keeps a single header so the phone still
        // gets a 0/1 frame and assembles to an empty blob (vs. nothing at all).
        let chunks = build_chunks(&[]);
        assert_eq!(chunks.len(), 1);
        let (total, idx, data) = parse(&chunks[0]);
        assert_eq!((total, idx), (1, 0));
        assert!(data.is_empty());
    }

    #[test]
    fn build_chunks_exact_multiple_has_no_phantom_tail() {
        // Exactly 2 chunks worth — must NOT emit a third empty chunk.
        let bytes = vec![7u8; 2 * PUSH_CHUNK_BYTES];
        let chunks = build_chunks(&bytes);
        assert_eq!(chunks.len(), 2);
        assert_eq!(parse(&chunks[0]).0, 2);
        assert_eq!(parse(&chunks[1]).1, 1);
    }

    /// Single test owns the global QUEUE so the assertions stay sequential
    /// (other test modules never touch it). Drains first for a clean slate.
    #[test]
    fn queue_is_fifo_and_bounded() {
        while take_batch().is_some() {}

        let f = |n: &str| OutgoingFile {
            name: n.into(),
            mime: "application/octet-stream".into(),
            bytes: vec![0u8; 8],
            extract: false,
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
    }

    #[test]
    fn enqueue_rejects_over_batch_cap() {
        let big = OutgoingFile {
            name: "huge".into(),
            mime: "application/octet-stream".into(),
            bytes: vec![0u8; MAX_BATCH_BYTES + 1],
            extract: false,
        };
        assert!(!enqueue_batch(vec![big]), "over MAX_BATCH_BYTES rejected");
    }
}
