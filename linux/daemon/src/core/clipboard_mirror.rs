//! Clipboard-sync payload (bidirectional, seamless universal-clipboard style).
//!
//! Text copied on one device, mirrored to the other over the Noise-sealed
//! BLE link (frame type `ty::CLIPBOARD`) — or LAN for larger payloads.
//! Deliberately tiny: just the text + a freshness timestamp. The sender
//! caps the length before sending; bodies are never logged verbatim.

use serde::{Deserialize, Serialize};

/// Hard cap on synced text (chars) — bounds memory and the BLE chunk storm.
/// Text up to [`MAX_SINGLE_FRAME_TEXT_BYTES`] rides one CLIPBOARD frame; longer
/// text is chunked over CLIPBOARD_TEXT. Matches the Android side's cap.
pub const MAX_CLIPBOARD_TEXT_CHARS: usize = 65_536;

/// Text whose UTF-8 form is this many bytes or fewer rides a single CLIPBOARD
/// (0x40) frame; anything longer is chunked over CLIPBOARD_TEXT (0x43) so every
/// frame stays under the BLE notify MTU (same reason images are chunked).
pub const MAX_SINGLE_FRAME_TEXT_BYTES: usize = 400;

/// Max bytes for a phone→laptop FILE pulled over LAN (reliable TCP). Files
/// ride the same offer+pull path as images but can be much larger; this bounds
/// memory and transfer time. ~64 MiB covers documents, photos, short clips.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// "Blob available, pull it over LAN" signal (phone→laptop). The laptop fetches
/// the bytes by `token` via the next bulk-sync (served as CLIPBOARD_IMAGE
/// chunks). When `name`/`mime` are EMPTY it's a clipboard IMAGE (PNG); when set
/// it's a FILE — the laptop writes it to disk with that name and makes it
/// pasteable (universal-clipboard file parity). One struct, two uses, so
/// the whole offer+pull pipeline is shared.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardImageOffer {
    /// Opaque id for the pending blob (the phone serves it by this token).
    #[serde(default)]
    pub token: String,
    /// Byte size — informational (UI / sanity / cap check).
    #[serde(default)]
    pub bytes: u64,
    /// File display name (with extension). EMPTY ⇒ this is an image, not a file.
    #[serde(default)]
    pub name: String,
    /// File MIME type. EMPTY ⇒ image.
    #[serde(default)]
    pub mime: String,
}

impl ClipboardImageOffer {
    /// True if this offer is a FILE (has a name) rather than a clipboard image.
    pub fn is_file(&self) -> bool {
        !self.name.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardMirror {
    /// The copied text (UTF-8). Capped at [`MAX_CLIPBOARD_TEXT_CHARS`].
    #[serde(default)]
    pub text: String,
    /// Origin timestamp (unix millis) — last-write-wins freshness hint.
    #[serde(default)]
    pub ts: u64,
}

impl ClipboardMirror {
    /// Build a capped clipboard payload from raw text.
    pub fn new(text: impl Into<String>, ts: u64) -> Self {
        let mut text = text.into();
        if text.chars().count() > MAX_CLIPBOARD_TEXT_CHARS {
            text = text.chars().take(MAX_CLIPBOARD_TEXT_CHARS).collect();
        }
        Self { text, ts }
    }
}

// --------------------------------------------------------------------------
// Image clipboard sync (chunked PNG over BLE), same `[total][idx][data]`
// wire as SMS. Larger images take the LAN path (future).
// --------------------------------------------------------------------------

/// PNG bytes per CLIPBOARD_IMAGE chunk — kept under the BLE notify MTU
/// (matches the ICON chunk size used elsewhere). Header is 4 bytes
/// (`[total u16][idx u16]`) + 16 AEAD + 4 frame ≈ fits a 512-MTU notify.
pub const IMAGE_CHUNK_BYTES: usize = 454;

/// Cap on a clipboard image synced over BLE (B1). ~95% of real screenshots
/// fit; larger ones wait for the LAN path. Bounds the BLE chunk storm.
pub const MAX_BLE_IMAGE_BYTES: usize = 1_048_576; // 1 MiB

const MAX_IMAGE_CHUNKS: u16 = 4096;

/// Split a PNG into `[total u16][idx u16][chunk]` payloads for the BLE
/// listener. Each is sealed + sent as its own CLIPBOARD_IMAGE frame.
pub fn build_image_chunks(png: &[u8]) -> Vec<Vec<u8>> {
    let total = png.len().div_ceil(IMAGE_CHUNK_BYTES).max(1) as u16;
    png.chunks(IMAGE_CHUNK_BYTES)
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

/// PNG/text bytes per CLIPBOARD_TEXT chunk — same MTU budget as images.
pub const TEXT_CHUNK_BYTES: usize = IMAGE_CHUNK_BYTES;

/// Split UTF-8 `text` into `[total u16][idx u16][utf8-chunk]` payloads for the
/// CLIPBOARD_TEXT path, NEVER splitting a multi-byte char across a chunk
/// boundary (so the reassembled concatenation is valid UTF-8). Chunks are
/// reassembled by [`ImageAssembler`] (byte-blind) and then `String::from_utf8`d.
pub fn build_text_chunks(text: &str) -> Vec<Vec<u8>> {
    // Group whole chars into ≤ TEXT_CHUNK_BYTES byte windows.
    let mut groups: Vec<&str> = Vec::new();
    let mut start = 0usize;
    for (i, ch) in text.char_indices() {
        if i > start && i - start + ch.len_utf8() > TEXT_CHUNK_BYTES {
            groups.push(&text[start..i]);
            start = i;
        }
    }
    if start < text.len() {
        groups.push(&text[start..]);
    }
    if groups.is_empty() {
        groups.push("");
    }
    let total = groups.len().max(1) as u16;
    groups
        .into_iter()
        .enumerate()
        .map(|(idx, g)| {
            let b = g.as_bytes();
            let mut out = Vec::with_capacity(4 + b.len());
            out.extend_from_slice(&total.to_be_bytes());
            out.extend_from_slice(&(idx as u16).to_be_bytes());
            out.extend_from_slice(b);
            out
        })
        .collect()
}

/// Reassembles CLIPBOARD_IMAGE chunks into the full PNG (same logic as
/// `SmsAssembler`, parsed via `core::sms::parse_chunk`).
#[derive(Default)]
pub struct ImageAssembler {
    total: u16,
    chunks: Vec<Option<Vec<u8>>>,
}

impl ImageAssembler {
    pub fn add(&mut self, total: u16, idx: u16, data: Vec<u8>) -> Option<Vec<u8>> {
        if total == 0 || total > MAX_IMAGE_CHUNKS || idx >= total {
            return None;
        }
        if self.total != total {
            self.total = total;
            self.chunks = vec![None; total as usize];
        }
        self.chunks[idx as usize] = Some(data);
        if self.chunks.iter().any(|c| c.is_none()) {
            return None;
        }
        let mut bytes = Vec::new();
        for c in &self.chunks {
            bytes.extend_from_slice(c.as_ref().unwrap());
        }
        self.total = 0;
        self.chunks = Vec::new();
        Some(bytes)
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::*;

    /// Parse a `[total u16 BE][idx u16 BE][data]` chunk into (total, idx, data).
    fn parse(c: &[u8]) -> (u16, u16, Vec<u8>) {
        (
            u16::from_be_bytes([c[0], c[1]]),
            u16::from_be_bytes([c[2], c[3]]),
            c[4..].to_vec(),
        )
    }

    /// Feed chunks (in the given order) through ImageAssembler; return the
    /// reassembled bytes once complete.
    fn reassemble(chunks: &[Vec<u8>]) -> Option<Vec<u8>> {
        let mut a = ImageAssembler::default();
        let mut out = None;
        for c in chunks {
            let (t, i, d) = parse(c);
            out = a.add(t, i, d);
        }
        out
    }

    #[test]
    fn image_chunks_roundtrip_and_header() {
        let png: Vec<u8> = (0..(IMAGE_CHUNK_BYTES * 2 + 7)).map(|i| (i % 251) as u8).collect();
        let chunks = build_image_chunks(&png);
        assert_eq!(chunks.len(), 3); // 2 full + 1 partial
        for (idx, c) in chunks.iter().enumerate() {
            let (t, i, _) = parse(c);
            assert_eq!(t as usize, chunks.len());
            assert_eq!(i as usize, idx); // monotonic 0..total
        }
        assert_eq!(reassemble(&chunks), Some(png));
    }

    #[test]
    fn text_chunks_never_split_a_codepoint() {
        // Cyrillic (2-byte) + emoji (4-byte) long enough to span many chunks.
        let text = "Салом дунё 🚀".repeat(4000);
        let chunks = build_text_chunks(&text);
        assert!(chunks.len() > 1, "must actually span multiple chunks");
        for c in &chunks {
            // Each chunk's payload must be valid UTF-8 on its own — i.e. no
            // multi-byte char was split across the boundary.
            assert!(std::str::from_utf8(&c[4..]).is_ok());
        }
        let bytes = reassemble(&chunks).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), text);
    }

    #[test]
    fn text_empty_yields_one_empty_chunk() {
        let chunks = build_text_chunks("");
        assert_eq!(chunks.len(), 1);
        assert_eq!(reassemble(&chunks), Some(Vec::new()));
    }

    #[test]
    fn assembler_rejects_malformed_indices() {
        let mut a = ImageAssembler::default();
        assert_eq!(a.add(0, 0, vec![1]), None); // total 0
        assert_eq!(a.add(MAX_IMAGE_CHUNKS + 1, 0, vec![1]), None); // over cap
        assert_eq!(a.add(2, 2, vec![1]), None); // idx >= total
        assert_eq!(a.add(2, 5, vec![1]), None); // idx way past total
    }

    #[test]
    fn assembler_handles_out_of_order_and_duplicate() {
        let mut a = ImageAssembler::default();
        assert_eq!(a.add(2, 1, vec![0xBB]), None); // second chunk first
        assert_eq!(a.add(2, 1, vec![0xBB]), None); // duplicate — still waiting
        assert_eq!(a.add(2, 0, vec![0xAA]), Some(vec![0xAA, 0xBB])); // completes in order
    }
}
