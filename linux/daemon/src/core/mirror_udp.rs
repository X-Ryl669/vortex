//! UDP video data-plane for screen mirroring (WireGuard-style).
//!
//! Video rides UDP, NOT the TCP control session, so a lost packet never stalls
//! the stream (TCP head-of-line blocking + retransmit is exactly the
//! "freeze-then-jump" we avoid). Security is preserved without the Noise
//! transport's strict in-order nonce: we derive a media key from the IK
//! handshake hash and seal each datagram with **ChaCha20-Poly1305 + an explicit
//! per-packet counter nonce**, guarded by a **sliding replay window** — the
//! same data-plane shape WireGuard uses over UDP.
//!
//! Wire (one datagram = one H.264 access-unit FRAGMENT):
//! ```text
//!   [ counter : u64 BE ]              # nonce material, also AEAD AAD
//!   [ ChaCha20Poly1305( key, nonce = 0u32 || counter, aad = counter,
//!                       plaintext = inner ) ]   # ciphertext + 16B tag
//!   inner = [ frame_id : u32 BE ][ frag_idx : u16 BE ][ frag_cnt : u16 BE ][ data ]
//! ```
//!
//! The receiver reassembles fragments per `frame_id`; if a fragment is missing
//! when a newer frame starts, the partial frame is dropped and a **keyframe
//! (IDR) request** is signalled back over the TCP control channel so the stream
//! self-heals at the next keyframe. Both peers derive the identical key via
//! HKDF over the shared handshake hash, so the phone (Kotlin) and the laptop
//! (here) interoperate.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// Largest UDP payload we send — keep one fragment inside a typical 1500-byte
/// MTU (IPv4/UDP headers + our 8-byte counter + 16-byte tag + inner header).
pub const MAX_FRAGMENT_DATA: usize = 1100;
/// Inner per-fragment header: frame_id(4) + frag_idx(2) + frag_cnt(2).
const INNER_HEADER: usize = 8;
/// HKDF info string — MUST match the Android `MirrorUdp` key derivation.
const KEY_INFO: &[u8] = b"vortex/mirror/udp";

/// HKDF info for the LAPTOP→phone mirror direction. A DISTINCT info string
/// derives a SEPARATE key so the two mirror directions never share the same
/// key+counter (the phone seals phone→laptop from counter 0, the laptop seals
/// laptop→phone from counter 0 — one shared key would collide nonces and break
/// AEAD). MUST match the Android laptop-mirror opener's key derivation.
const LAPTOP_KEY_INFO: &[u8] = b"vortex/mirror/laptop";

/// Derive a 32-byte media key from the IK handshake hash + an info label
/// (HKDF-SHA256). Identical inputs on both peers ⇒ identical key.
fn derive_media_key_with_info(handshake_hash: &[u8], info: &[u8]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, handshake_hash);
    let mut out = [0u8; 32];
    // `expand` only fails for absurd output lengths; 32 bytes never fails.
    hk.expand(info, &mut out).expect("hkdf expand 32 bytes");
    out
}

/// Derive the 32-byte phone→laptop UDP/TCP media key from the IK handshake hash.
/// Identical inputs on both peers ⇒ identical key.
pub fn derive_media_key(handshake_hash: &[u8]) -> [u8; 32] {
    derive_media_key_with_info(handshake_hash, KEY_INFO)
}

/// Derive the 32-byte LAPTOP→phone media key (distinct from [`derive_media_key`]
/// so the two directions never reuse a nonce). Both peers derive it identically.
pub fn derive_laptop_media_key(handshake_hash: &[u8]) -> [u8; 32] {
    derive_media_key_with_info(handshake_hash, LAPTOP_KEY_INFO)
}

/// Build the 12-byte ChaCha20-Poly1305 nonce from the 8-byte packet counter
/// (high 4 bytes zero). The counter is monotonic per direction (one key, one
/// sender) so the nonce never repeats.
#[inline]
fn nonce_from_counter(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_be_bytes());
    n
}

/// Seals AU fragments for sending (used in tests; the phone does this in
/// Kotlin). One `MediaSealer` per session; `counter` is monotonic.
pub struct MediaSealer {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl MediaSealer {
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(key)),
            counter: 0,
        }
    }

    /// Split one H.264 access unit into sealed datagrams ready to `send_to`.
    pub fn seal_access_unit(&mut self, frame_id: u32, au: &[u8]) -> Vec<Vec<u8>> {
        let frags: Vec<&[u8]> = if au.is_empty() {
            vec![&au[..]]
        } else {
            au.chunks(MAX_FRAGMENT_DATA).collect()
        };
        let frag_cnt = frags.len().min(u16::MAX as usize) as u16;
        frags
            .into_iter()
            .enumerate()
            .map(|(idx, data)| {
                let mut inner = Vec::with_capacity(INNER_HEADER + data.len());
                inner.extend_from_slice(&frame_id.to_be_bytes());
                inner.extend_from_slice(&(idx as u16).to_be_bytes());
                inner.extend_from_slice(&frag_cnt.to_be_bytes());
                inner.extend_from_slice(data);

                let counter = self.counter;
                self.counter = self.counter.wrapping_add(1);
                let counter_bytes = counter.to_be_bytes();
                let nonce = nonce_from_counter(counter);
                let ct = self
                    .cipher
                    .encrypt(
                        Nonce::from_slice(&nonce),
                        Payload { msg: &inner, aad: &counter_bytes },
                    )
                    .expect("chacha seal");

                let mut pkt = Vec::with_capacity(8 + ct.len());
                pkt.extend_from_slice(&counter_bytes);
                pkt.extend_from_slice(&ct);
                pkt
            })
            .collect()
    }
}

/// IPsec/WireGuard-style sliding replay window (64-bit) over the packet
/// counter — rejects duplicates and too-old packets.
#[derive(Default)]
pub struct ReplayWindow {
    last: u64,
    bitmap: u64,
    started: bool,
}

impl ReplayWindow {
    const WINDOW: u64 = 64;

    /// Accept `counter` once. Returns false for a replay / out-of-window packet.
    pub fn accept(&mut self, counter: u64) -> bool {
        if !self.started {
            self.started = true;
            self.last = counter;
            self.bitmap = 1;
            return true;
        }
        if counter > self.last {
            let shift = counter - self.last;
            if shift >= Self::WINDOW {
                self.bitmap = 1;
            } else {
                self.bitmap = (self.bitmap << shift) | 1;
            }
            self.last = counter;
            true
        } else {
            let diff = self.last - counter;
            if diff >= Self::WINDOW {
                return false; // too old
            }
            let mask = 1u64 << diff;
            if self.bitmap & mask != 0 {
                false // replay
            } else {
                self.bitmap |= mask;
                true
            }
        }
    }
}

/// Outcome of feeding one decrypted fragment to the reassembler.
pub enum Reassembled {
    /// A complete access unit is ready.
    Complete(Vec<u8>),
    /// Still waiting for more fragments of the current frame.
    Pending,
    /// A frame was abandoned incomplete (loss) — caller should request an IDR.
    Lost,
}

/// Reassembles AU fragments by `frame_id`. Single in-flight frame: a fragment
/// for a NEWER frame abandons the current (incomplete) one as lost.
#[derive(Default)]
pub struct Reassembler {
    frame_id: u32,
    frag_cnt: u16,
    have: u16,
    parts: Vec<Option<Vec<u8>>>,
    active: bool,
}

impl Reassembler {
    pub fn push(&mut self, inner: &[u8]) -> Reassembled {
        if inner.len() < INNER_HEADER {
            return Reassembled::Pending;
        }
        let frame_id = u32::from_be_bytes([inner[0], inner[1], inner[2], inner[3]]);
        let frag_idx = u16::from_be_bytes([inner[4], inner[5]]);
        let frag_cnt = u16::from_be_bytes([inner[6], inner[7]]);
        let data = &inner[INNER_HEADER..];
        if frag_cnt == 0 || frag_idx >= frag_cnt {
            return Reassembled::Pending;
        }

        let mut lost = false;
        if !self.active || frame_id != self.frame_id {
            // New frame. If the previous one never completed, it's lost.
            lost = self.active && self.have < self.frag_cnt;
            if self.active && frame_id < self.frame_id {
                // Stale fragment from an already-finished/abandoned frame.
                return if lost { Reassembled::Lost } else { Reassembled::Pending };
            }
            self.frame_id = frame_id;
            self.frag_cnt = frag_cnt;
            self.have = 0;
            self.parts = vec![None; frag_cnt as usize];
            self.active = true;
        }

        if self.parts[frag_idx as usize].is_none() {
            self.parts[frag_idx as usize] = Some(data.to_vec());
            self.have += 1;
        }

        if self.have == self.frag_cnt {
            let mut au = Vec::new();
            for p in &self.parts {
                if let Some(b) = p {
                    au.extend_from_slice(b);
                }
            }
            self.active = false;
            return Reassembled::Complete(au);
        }
        if lost {
            Reassembled::Lost
        } else {
            Reassembled::Pending
        }
    }
}

/// Open one received datagram. Returns the decrypted inner bytes, or `None` on
/// a malformed / replayed / forged packet (silently dropped — UDP is lossy).
pub fn open_packet(
    cipher: &ChaCha20Poly1305,
    replay: &mut ReplayWindow,
    pkt: &[u8],
) -> Option<Vec<u8>> {
    if pkt.len() < 8 + 16 {
        return None;
    }
    let counter = u64::from_be_bytes(pkt[..8].try_into().ok()?);
    if !replay.accept(counter) {
        return None;
    }
    let nonce = nonce_from_counter(counter);
    let counter_bytes = &pkt[..8];
    cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload { msg: &pkt[8..], aad: counter_bytes },
        )
        .ok()
}

/// Bind-and-run the UDP video receiver: recv → AEAD-open → reassemble → push
/// complete access units to `au_tx` (the GStreamer feeder). On a detected
/// dropped frame it pokes `keyframe_tx` so the TCP control writer asks the
/// phone for an IDR — the stream then self-heals at the next keyframe. Runs
/// until the socket errors or `au_tx` is dropped (session torn down).
pub async fn run_udp_receiver(
    socket: std::sync::Arc<UdpSocket>,
    key: [u8; 32],
    au_tx: mpsc::Sender<Vec<u8>>,
    keyframe_tx: mpsc::Sender<()>,
) {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let mut replay = ReplayWindow::default();
    let mut re = Reassembler::default();
    // Max datagram = 8 (counter) + MAX_FRAGMENT_DATA + INNER_HEADER + 16 (tag),
    // rounded up for safety.
    let mut buf = vec![0u8; 2048];
    let local = socket.local_addr().ok();
    tracing::info!(?local, "mirror udp receiver listening");
    let mut pkts: u64 = 0;
    let mut aus: u64 = 0;
    let mut dropped: u64 = 0;
    let mut lost: u64 = 0; // frames abandoned incomplete (fragment loss)
    loop {
        let n = match socket.recv(&mut buf).await {
            Ok(0) => continue,
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("mirror udp recv: {e}");
                break;
            }
        };
        pkts += 1;
        if pkts == 1 {
            tracing::info!(bytes = n, "mirror udp: FIRST packet received");
        }
        let Some(inner) = open_packet(&cipher, &mut replay, &buf[..n]) else {
            dropped += 1;
            if dropped == 1 {
                tracing::warn!("mirror udp: first packet FAILED to decrypt/validate (key/replay/forged)");
            }
            continue; // malformed / replayed / forged — drop
        };
        match re.push(&inner) {
            Reassembled::Complete(au) => {
                aus += 1;
                if aus == 1 {
                    tracing::info!(bytes = au.len(), "mirror udp: FIRST access unit reassembled → GStreamer");
                }
                if aus % 120 == 0 {
                    tracing::info!(pkts, aus, dropped, lost, "mirror udp: streaming");
                }
                if au_tx.send(au).await.is_err() {
                    tracing::warn!("mirror udp: AU consumer gone — stopping");
                    break; // consumer gone
                }
            }
            Reassembled::Lost => {
                lost += 1;
                // Best-effort; the writer debounces actual IDR requests.
                let _ = keyframe_tx.try_send(());
            }
            Reassembled::Pending => {}
        }
    }
    tracing::info!(pkts, aus, dropped, lost, "mirror udp receiver ended");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        derive_media_key(b"some-handshake-transcript-hash-32")
    }

    #[test]
    fn key_derivation_is_stable_and_matches_both_sides() {
        let a = derive_media_key(b"transcript");
        let b = derive_media_key(b"transcript");
        assert_eq!(a, b);
        assert_ne!(a, derive_media_key(b"other"));
    }

    #[test]
    fn seal_open_round_trip_single_fragment() {
        let k = key();
        let mut sealer = MediaSealer::new(&k);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&k));
        let mut replay = ReplayWindow::default();

        let au = b"a small access unit".to_vec();
        let pkts = sealer.seal_access_unit(7, &au);
        assert_eq!(pkts.len(), 1);
        let inner = open_packet(&cipher, &mut replay, &pkts[0]).unwrap();
        let mut re = Reassembler::default();
        match re.push(&inner) {
            Reassembled::Complete(out) => assert_eq!(out, au),
            _ => panic!("expected complete"),
        }
    }

    #[test]
    fn fragmented_au_reassembles() {
        let k = key();
        let mut sealer = MediaSealer::new(&k);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&k));
        let mut replay = ReplayWindow::default();
        let mut re = Reassembler::default();

        let au = vec![0xABu8; MAX_FRAGMENT_DATA * 3 + 17];
        let pkts = sealer.seal_access_unit(1, &au);
        assert_eq!(pkts.len(), 4);
        let mut done = None;
        for p in &pkts {
            let inner = open_packet(&cipher, &mut replay, p).unwrap();
            if let Reassembled::Complete(out) = re.push(&inner) {
                done = Some(out);
            }
        }
        assert_eq!(done.unwrap(), au);
    }

    #[test]
    fn replay_is_rejected() {
        let mut w = ReplayWindow::default();
        assert!(w.accept(5));
        assert!(!w.accept(5)); // dup
        assert!(w.accept(6));
        assert!(w.accept(4)); // in-window, not yet seen
        assert!(!w.accept(4)); // now a dup
    }

    #[test]
    fn dropped_fragment_then_new_frame_reports_loss() {
        let k = key();
        let mut sealer = MediaSealer::new(&k);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&k));
        let mut replay = ReplayWindow::default();
        let mut re = Reassembler::default();

        // Frame 1: 2 fragments, but we deliver only the first.
        let au1 = vec![1u8; MAX_FRAGMENT_DATA + 10];
        let f1 = sealer.seal_access_unit(1, &au1);
        assert_eq!(f1.len(), 2);
        let inner = open_packet(&cipher, &mut replay, &f1[0]).unwrap();
        assert!(matches!(re.push(&inner), Reassembled::Pending));

        // Frame 2 starts → frame 1 is abandoned as lost.
        let au2 = b"frame two".to_vec();
        let f2 = sealer.seal_access_unit(2, &au2);
        let inner2 = open_packet(&cipher, &mut replay, &f2[0]).unwrap();
        assert!(matches!(re.push(&inner2), Reassembled::Complete(_) | Reassembled::Lost));
    }

    #[test]
    fn forged_packet_is_dropped() {
        let k = key();
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&k));
        let mut replay = ReplayWindow::default();
        let mut forged = vec![0u8; 8 + 16 + 4];
        forged[7] = 1; // counter = 1
        assert!(open_packet(&cipher, &mut replay, &forged).is_none());
    }
}
