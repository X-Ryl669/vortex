//! Recent call-log mirror (phone → laptop companion Recents page).
//!
//! Same chunked transfer as contacts: the phone sends its recent calls as a
//! JSON array split across `ty::CALL_LOG` frames; we reassemble the single
//! stream and the UI layer caches it (`~/.cache/vortex/call_log.json`). Mirrors
//! Kotlin `core::calllog::CallLogEntry`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallLogEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub number: String,
    #[serde(default)]
    pub name: String,
    /// `CallLog.Calls.TYPE`: 1=incoming, 2=outgoing, 3=missed, 4=voicemail,
    /// 5=rejected, 6=blocked.
    #[serde(default)]
    pub r#type: i32,
    /// Epoch milliseconds.
    #[serde(default)]
    pub date: i64,
    /// Seconds.
    #[serde(default)]
    pub duration: i64,
}

/// Parse a CALL_LOG frame plaintext: `[total u16 BE][idx u16 BE][json-chunk]`.
pub fn parse_chunk(plain: &[u8]) -> Option<(u16, u16, Vec<u8>)> {
    if plain.len() < 4 {
        return None;
    }
    let total = u16::from_be_bytes([plain[0], plain[1]]);
    let idx = u16::from_be_bytes([plain[2], plain[3]]);
    Some((total, idx, plain[4..].to_vec()))
}

/// Upper bound on declared chunk counts — same rationale as
/// `contacts::MAX_CHUNKS`: the recent-calls list is a handful of 450-byte
/// chunks, so a larger declared total is hostile/corrupt and must not drive
/// the buffer allocation.
pub const MAX_CHUNKS: u16 = 2048;

/// Reassembles the single call-log JSON stream from its chunks. Returns the full
/// JSON bytes once every chunk has arrived. A re-send with a different chunk
/// count (the log changed) restarts the buffer.
#[derive(Default)]
pub struct CallLogAssembler {
    total: u16,
    chunks: Vec<Option<Vec<u8>>>,
}

impl CallLogAssembler {
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
        let mut asm = CallLogAssembler::default();
        assert!(asm.add(MAX_CHUNKS + 1, 0, b"x".to_vec()).is_none());
        assert_eq!(asm.add(1, 0, b"ok".to_vec()), Some(b"ok".to_vec()));
    }
}
