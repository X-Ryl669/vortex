//! Contacts mirror (phone → laptop companion Contacts page).
//!
//! The phone sends its full contacts list as a JSON array, split across
//! `ty::CONTACTS` frames (BLE notifies are small) — chunked like icons. We
//! reassemble the single stream, then the UI layer caches it
//! (`~/.cache/vortex/contacts.json`) and renders the Contacts page. Mirrors
//! Kotlin `core::contacts::Contact`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contact {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub numbers: Vec<String>,
}

/// Parse a CONTACTS frame plaintext: `[total u16 BE][idx u16 BE][json-chunk]`.
pub fn parse_chunk(plain: &[u8]) -> Option<(u16, u16, Vec<u8>)> {
    if plain.len() < 4 {
        return None;
    }
    let total = u16::from_be_bytes([plain[0], plain[1]]);
    let idx = u16::from_be_bytes([plain[2], plain[3]]);
    Some((total, idx, plain[4..].to_vec()))
}

/// Upper bound on declared chunk counts. The largest real stream (a big
/// contacts list) is a few hundred 450-byte chunks; anything past this is a
/// hostile/corrupt header and must not drive the buffer allocation.
pub const MAX_CHUNKS: u16 = 2048;

/// Reassembles the single contacts JSON stream from its chunks. Returns the
/// full JSON bytes once every chunk has arrived. A re-send with a different
/// chunk count (the list changed) restarts the buffer.
#[derive(Default)]
pub struct ContactsAssembler {
    total: u16,
    chunks: Vec<Option<Vec<u8>>>,
}

impl ContactsAssembler {
    pub fn add(&mut self, total: u16, idx: u16, data: Vec<u8>) -> Option<Vec<u8>> {
        if total == 0 || total > MAX_CHUNKS || idx >= total {
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
        // Reset so the next full re-send rebuilds cleanly.
        self.total = 0;
        self.chunks = Vec::new();
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_in_any_order() {
        let mut asm = ContactsAssembler::default();
        assert!(asm.add(2, 1, b"b".to_vec()).is_none());
        assert_eq!(asm.add(2, 0, b"a".to_vec()), Some(b"ab".to_vec()));
    }

    #[test]
    fn rejects_total_above_cap() {
        let mut asm = ContactsAssembler::default();
        assert!(asm.add(MAX_CHUNKS + 1, 0, b"x".to_vec()).is_none());
        assert!(asm.add(u16::MAX, 0, b"x".to_vec()).is_none());
        // The hostile header must not have poisoned the buffer.
        assert_eq!(asm.add(1, 0, b"ok".to_vec()), Some(b"ok".to_vec()));
    }
}
