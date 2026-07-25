//! SAS derivation per spec §6.5.1.
//!
//! ```text
//! sas_full   = HMAC-SHA256(key = "vortex/v1/sas", data = h)
//! sas_bytes  = sas_full[0..4]                                    // 4 bytes
//! sas_int    = u32_be(sas_bytes)
//! sas_value  = sas_int mod 1_000_000
//! sas_string = format("%06d", sas_value)
//! ```
//!
//! The 4-byte → mod-10^6 reduction introduces ~2.3 × 10^-5 bias toward
//! lower values. Below any practical detection threshold for a human-
//! readable 6-digit code and well below SAS guessing margin in a 60s
//! approval window. Vector fixtures lock this exact derivation in.

use super::kdf::hmac_sha256;

pub const SAS_LABEL: &[u8] = b"vortex/v1/sas";

/// Derive the 6-digit SAS string and its integer value from the
/// post-handshake transcript hash `h`.
///
/// Returns `(sas_value_int, sas_string)` where:
/// - `sas_value_int ∈ 0..1_000_000`
/// - `sas_string` is the zero-padded 6-digit decimal form (`"000000"`..`"999999"`)
pub fn derive_sas(transcript_hash: &[u8]) -> (u32, String) {
    let sas_full = hmac_sha256(SAS_LABEL, transcript_hash);
    let sas_int = u32::from_be_bytes([sas_full[0], sas_full[1], sas_full[2], sas_full[3]]);
    let sas_value = sas_int % 1_000_000;
    (sas_value, format!("{:06}", sas_value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_zero_transcript() {
        let h = [0u8; 32];
        let (v, s) = derive_sas(&h);
        // Stable output for all-zero h: only depends on
        // HMAC-SHA256("vortex/v1/sas", 32 zero bytes).
        let expected = hmac_sha256(SAS_LABEL, &h);
        let raw =
            u32::from_be_bytes([expected[0], expected[1], expected[2], expected[3]]) % 1_000_000;
        assert_eq!(v, raw);
        assert_eq!(s, format!("{:06}", raw));
    }

    #[test]
    fn one_byte_change_changes_output() {
        let mut h = [0u8; 32];
        let (a, _) = derive_sas(&h);
        h[0] = 1;
        let (b, _) = derive_sas(&h);
        assert_ne!(a, b);
    }

    #[test]
    fn output_is_six_digits() {
        for seed in 0..50u8 {
            let mut h = [0u8; 32];
            h[0] = seed;
            let (_, s) = derive_sas(&h);
            assert_eq!(s.len(), 6, "{s} must be exactly 6 chars");
            assert!(s.chars().all(|c| c.is_ascii_digit()));
        }
    }
}
