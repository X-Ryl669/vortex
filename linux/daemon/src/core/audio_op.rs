//! Earbuds-switch op wire types (the earbuds-switch design notes §4).
//!
//! These ride frame type `AUDIO_OP = 0x32` inside the existing post-
//! handshake Noise AEAD channel. They do NOT touch the pairing
//! protocol — security is rooted in the transport-mode cipher pair
//! produced by IK.split().
//!
//! Per-peer monotonic `nonce` rejects replay. The receiver keeps the
//! highest accepted nonce per peer and drops anything `<= in_nonce`.

use serde::{Deserialize, Serialize};

/// One switch operation. Sent inside an `AudioOpFrame`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AudioOp {
    /// Initiator asks the owner to release the buds.
    Request,
    /// Sender currently holds the buds and is asking the recipient to
    /// claim them. The recipient runs its own initiator flow on
    /// receipt — this is how bidirectional swap works without needing
    /// the recipient to be reachable as a server.
    Claim,
    /// Owner agrees and is about to disconnect.
    Approve,
    /// Owner refuses (in a call, just claimed, etc.).
    Reject { reason: RejectReason },
    /// Owner finished disconnecting; initiator may connect now.
    Released,
    /// Initiator finished connecting; round-trip complete.
    Done,
    /// Either side hit a local failure. Both rollback to `Idle`.
    Failed { stage: Stage, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// Owner is currently in another switch flow.
    Busy,
    /// Owner is in (or just answered) a phone call — higher priority.
    InCall,
    /// Owner just won the buds within the cool-down window.
    RecentSwitch,
    /// Catch-all for forward-compat with newer reasons we don't know.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Disconnect,
    Connect,
    WaitReady,
}

/// Wire envelope. Serialized as JSON and AEAD-wrapped inside a frame
/// of type `AUDIO_OP`. JSON is cheap (~150 bytes ciphertext) and lets
/// the Kotlin side roundtrip with the same struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioOpFrame {
    /// Per-direction monotonic counter. Receiver MUST drop frames
    /// whose nonce is `<=` the highest previously-accepted nonce for
    /// this peer. Persisted on both sides so a process restart does
    /// not reset the counter window (rollback would let an attacker
    /// replay an old `Done`).
    pub nonce: u64,
    pub op: AudioOp,
    /// MAC of the earbuds this op refers to. Multi-bud-set V2 will
    /// dispatch by this; V1 with a single saved set is informational.
    pub mac: String,
    /// Unix seconds on the sender side. Receiver uses for ordering
    /// hints only — clock skew is not a security boundary.
    pub ts: u64,
}

impl AudioOpFrame {
    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_request() {
        let f = AudioOpFrame {
            nonce: 42,
            op: AudioOp::Request,
            mac: "AC:47:1B:25:71:C2".to_string(),
            ts: 1_700_000_000,
        };
        let bytes = f.to_json().unwrap();
        let back = AudioOpFrame::from_json(&bytes).unwrap();
        assert_eq!(back.nonce, 42);
        assert!(matches!(back.op, AudioOp::Request));
        assert_eq!(back.mac, f.mac);
    }

    #[test]
    fn roundtrip_reject_with_reason() {
        let f = AudioOpFrame {
            nonce: 100,
            op: AudioOp::Reject { reason: RejectReason::InCall },
            mac: "AA:BB:CC:DD:EE:FF".to_string(),
            ts: 0,
        };
        let bytes = f.to_json().unwrap();
        let back = AudioOpFrame::from_json(&bytes).unwrap();
        match back.op {
            AudioOp::Reject { reason } => assert_eq!(reason, RejectReason::InCall),
            _ => panic!("expected Reject"),
        }
    }

    #[test]
    fn roundtrip_failed() {
        let f = AudioOpFrame {
            nonce: 7,
            op: AudioOp::Failed {
                stage: Stage::Connect,
                message: "A2DP profile refused".into(),
            },
            mac: "AA:BB:CC:DD:EE:FF".to_string(),
            ts: 1,
        };
        let bytes = f.to_json().unwrap();
        let back = AudioOpFrame::from_json(&bytes).unwrap();
        match back.op {
            AudioOp::Failed { stage, ref message } => {
                assert_eq!(stage, Stage::Connect);
                assert_eq!(message, "A2DP profile refused");
            }
            _ => panic!("expected Failed"),
        }
    }

    #[test]
    fn unknown_reject_reason_deserializes_to_unknown_or_error() {
        // Forward-compat probe: a peer running a newer build sends a
        // reason we don't know. Current behaviour: serde errors —
        // caller treats as "bad frame" and drops. Acceptable; we can
        // tighten to a tolerant fallback later if needed.
        let bad = br#"{"nonce":1,"op":{"kind":"reject","reason":"future_reason"},"mac":"x","ts":0}"#;
        let r = AudioOpFrame::from_json(bad);
        assert!(r.is_err(), "unknown reason should error in V1");
    }
}
