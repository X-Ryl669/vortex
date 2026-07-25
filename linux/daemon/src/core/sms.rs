//! SMS mirror (phone → laptop companion Messages page).
//!
//! Same chunked transfer as contacts/call-log: the phone sends its recent SMS
//! as a JSON array split across `ty::SMS` frames; we reassemble the single
//! stream and the UI layer caches it (`~/.cache/vortex/sms.json`) + groups it
//! into conversations. Bodies are sensitive — never logged. Mirrors Kotlin
//! `core::sms::SmsMessage`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmsMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub body: String,
    /// `Telephony.Sms.TYPE`: 1=inbox (received), 2=sent.
    #[serde(default)]
    pub r#type: i32,
    /// Epoch milliseconds.
    #[serde(default)]
    pub date: i64,
    /// Conversation thread id.
    #[serde(default)]
    pub thread: i64,
    /// `Telephony.Sms.READ`: 0=unread, 1=read.
    #[serde(default)]
    pub read: i32,
}

/// Parse an SMS frame plaintext: `[total u16 BE][idx u16 BE][json-chunk]`.
pub fn parse_chunk(plain: &[u8]) -> Option<(u16, u16, Vec<u8>)> {
    if plain.len() < 4 {
        return None;
    }
    let total = u16::from_be_bytes([plain[0], plain[1]]);
    let idx = u16::from_be_bytes([plain[2], plain[3]]);
    Some((total, idx, plain[4..].to_vec()))
}

/// Upper bound on declared chunk counts — same rationale as
/// `contacts::MAX_CHUNKS`: the recent-SMS list is well under a hundred
/// 450-byte chunks, so a larger declared total is hostile/corrupt and must
/// not drive the buffer allocation.
pub const MAX_CHUNKS: u16 = 2048;

/// Reassembles the single SMS JSON stream from its chunks. Returns the full JSON
/// bytes once every chunk has arrived. A re-send with a different chunk count
/// restarts the buffer.
#[derive(Default)]
pub struct SmsAssembler {
    total: u16,
    chunks: Vec<Option<Vec<u8>>>,
}

impl SmsAssembler {
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
        self.total = 0;
        self.chunks = Vec::new();
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_total_above_cap() {
        let mut asm = SmsAssembler::default();
        assert!(asm.add(MAX_CHUNKS + 1, 0, b"x".to_vec()).is_none());
        assert_eq!(asm.add(1, 0, b"ok".to_vec()), Some(b"ok".to_vec()));
    }
}
