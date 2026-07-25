//! Generate canonical V1 test vectors per the test-vectors doc.
//!
//! Outputs JSON files into shared/vectors/v1/.

use serde_json::json;
use std::fs;
use std::path::PathBuf;

use vortex_l3_daemon::core::crypto::{derive, noise, presence, sas, x25519};

fn main() {
    let out_dir = workspace_root().join("shared/vectors/v1");
    fs::create_dir_all(&out_dir).expect("create shared/vectors/v1");

    write_sas_vectors(&out_dir);
    write_prs_vectors(&out_dir);
    write_ses_vectors(&out_dir);
    write_presence_vectors(&out_dir);
    write_noise_xx_vectors(&out_dir);
    write_noise_ik_vectors(&out_dir);
    write_x25519_vectors(&out_dir);

    println!("vectors written to {}", out_dir.display());
}

fn write_x25519_vectors(dir: &PathBuf) {
    // RFC 7748 §6.1 Alice keypair plus a couple of synthetic seeds.
    let alice_priv: [u8; 32] = [
        0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d, 0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2, 0x66,
        0x45, 0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a, 0xb1, 0x77, 0xfb, 0xa5, 0x1d, 0xb9,
        0x2c, 0x2a,
    ];
    let mut sequential = [0u8; 32];
    for (i, b) in sequential.iter_mut().enumerate() {
        *b = i as u8;
    }
    let cases: Vec<_> = [alice_priv, [0u8; 32], sequential]
        .iter()
        .map(|priv_bytes| {
            let pub_bytes = x25519::public_from_private(priv_bytes);
            json!({
                "private_hex": hex::encode(priv_bytes),
                "public_hex":  hex::encode(pub_bytes),
            })
        })
        .collect();

    write_json(
        dir.join("x25519.json"),
        json!({
            "vector_set": "vortex/v1/x25519",
            "version":    1,
            "comment":    "X25519 public-key derivation per spec §3 and §4.1.",
            "algorithm":  "RFC 7748 X25519 base-point multiplication",
            "cases":      cases,
        }),
    );
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at linux/daemon. Workspace root is two above.
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).parent().unwrap().parent().unwrap().to_path_buf()
}

fn write_json(path: PathBuf, value: serde_json::Value) {
    let pretty = serde_json::to_string_pretty(&value).unwrap();
    fs::write(&path, pretty + "\n").expect("write vector file");
    println!("  {}", path.file_name().unwrap().to_string_lossy());
}

fn write_sas_vectors(dir: &PathBuf) {
    let cases: Vec<_> = [[0u8; 32], [0xFFu8; 32], {
        let mut h = [0u8; 32];
        for (i, b) in h.iter_mut().enumerate() {
            *b = i as u8;
        }
        h
    }]
    .iter()
    .map(|h| {
        let (val, s) = sas::derive_sas(h);
        json!({
            "h_hex":      hex::encode(h),
            "sas_value":  val,
            "sas_string": s,
        })
    })
    .collect();

    write_json(
        dir.join("sas.json"),
        json!({
            "vector_set": "vortex/v1/sas",
            "version":    1,
            "comment":    "SAS derivation per spec §6.5.1.",
            "label":      "vortex/v1/sas",
            "algorithm":  "HMAC-SHA256(label, h)[0..4] mod 1_000_000, format %06d",
            "cases":      cases,
        }),
    );
}

fn write_prs_vectors(dir: &PathBuf) {
    let cks = [[0u8; 32], [0x55u8; 32], [0xAAu8; 32]];
    let cases: Vec<_> = cks
        .iter()
        .map(|ck| {
            let prs = derive::derive_prs(ck);
            json!({
                "ck_hex":  hex::encode(ck),
                "prs_hex": hex::encode(prs),
            })
        })
        .collect();

    write_json(
        dir.join("prs.json"),
        json!({
            "vector_set": "vortex/v1/prs",
            "version":    1,
            "comment":    "Pairwise Reconnect Secret per spec §6.5.2.",
            "label":      "vortex/v1/prs",
            "algorithm":  "HMAC-SHA256(label, ck)",
            "cases":      cases,
        }),
    );
}

fn write_ses_vectors(dir: &PathBuf) {
    let hs = [[0u8; 32], [0x33u8; 32], [0xCCu8; 32]];
    let cases: Vec<_> = hs
        .iter()
        .map(|h| {
            let ses = derive::derive_ses(h);
            json!({
                "h_hex":   hex::encode(h),
                "ses_hex": hex::encode(ses),
            })
        })
        .collect();

    write_json(
        dir.join("ses.json"),
        json!({
            "vector_set": "vortex/v1/ses",
            "version":    1,
            "comment":    "Session Export Secret per spec §6.5.3.",
            "label":      "vortex/v1/ses",
            "algorithm":  "HMAC-SHA256(label, h)",
            "cases":      cases,
        }),
    );
}

fn write_presence_vectors(dir: &PathBuf) {
    let mut prs1 = [0u8; 32];
    for (i, b) in prs1.iter_mut().enumerate() {
        *b = i as u8;
    }
    let buckets: [u64; 4] = [0, 1, 1_000_000, 28_633_333];
    let cases: Vec<_> = buckets
        .iter()
        .map(|&bucket| {
            let token = presence::derive_presence_token(&prs1, bucket);
            json!({
                "prs_hex":   hex::encode(prs1),
                "bucket":    bucket,
                "token_hex": hex::encode(token),
            })
        })
        .collect();

    write_json(
        dir.join("presence.json"),
        json!({
            "vector_set": "vortex/v1/presence",
            "version":    1,
            "comment":    "Private Presence Token per spec §7.3.",
            "label":      "vortex/v1/presence",
            "algorithm":  "HMAC-SHA256(prs, label || u64_be(bucket))[0..8]",
            "cases":      cases,
        }),
    );
}

fn write_noise_xx_vectors(dir: &PathBuf) {
    let init_s: [u8; 32] = std::array::from_fn(|i| 0x10 + i as u8);
    let resp_s: [u8; 32] = std::array::from_fn(|i| 0x30 + i as u8);
    let init_e: [u8; 32] = std::array::from_fn(|i| 0x50 + i as u8);
    let resp_e: [u8; 32] = std::array::from_fn(|i| 0x70 + i as u8);

    let r = noise::run_xx_deterministic(&init_s, &resp_s, &init_e, &resp_e)
        .expect("XX runs");

    write_json(
        dir.join("noise-xx.json"),
        json!({
            "vector_set": "vortex/v1/noise-xx",
            "version":    1,
            "comment":    "Noise XX wire vector per spec §6.4. Empty handshake payload.",
            "noise_pattern": noise::NOISE_XX,
            "prologue_hex":  hex::encode(noise::PROLOGUE_XX),
            "inputs": {
                "initiator_static_priv_hex":    hex::encode(init_s),
                "responder_static_priv_hex":    hex::encode(resp_s),
                "initiator_ephemeral_priv_hex": hex::encode(init_e),
                "responder_ephemeral_priv_hex": hex::encode(resp_e),
            },
            "outputs": {
                "msg1_hex": hex::encode(&r.messages[0]),
                "msg2_hex": hex::encode(&r.messages[1]),
                "msg3_hex": hex::encode(&r.messages[2]),
                "transcript_hash_hex": hex::encode(&r.initiator_handshake_hash),
            },
        }),
    );
}

fn write_noise_ik_vectors(dir: &PathBuf) {
    let init_s: [u8; 32] = std::array::from_fn(|i| 0x10 + i as u8);
    let resp_s: [u8; 32] = std::array::from_fn(|i| 0x30 + i as u8);
    let init_e: [u8; 32] = std::array::from_fn(|i| 0x50 + i as u8);
    let resp_e: [u8; 32] = std::array::from_fn(|i| 0x70 + i as u8);

    let resp_sk = x25519_dalek::StaticSecret::from(resp_s);
    let resp_pub = x25519_dalek::PublicKey::from(&resp_sk).to_bytes();

    let r = noise::run_ik_deterministic(&init_s, &resp_s, &init_e, &resp_e, &resp_pub)
        .expect("IK runs");

    write_json(
        dir.join("noise-ik.json"),
        json!({
            "vector_set": "vortex/v1/noise-ik",
            "version":    1,
            "comment":    "Noise IK wire vector per spec §7.4. Empty handshake payload.",
            "noise_pattern": noise::NOISE_IK,
            "prologue_hex":  hex::encode(noise::PROLOGUE_IK),
            "inputs": {
                "initiator_static_priv_hex":    hex::encode(init_s),
                "responder_static_priv_hex":    hex::encode(resp_s),
                "responder_static_pub_hex":     hex::encode(resp_pub),
                "initiator_ephemeral_priv_hex": hex::encode(init_e),
                "responder_ephemeral_priv_hex": hex::encode(resp_e),
            },
            "outputs": {
                "msg1_hex": hex::encode(&r.messages[0]),
                "msg2_hex": hex::encode(&r.messages[1]),
                "transcript_hash_hex": hex::encode(&r.initiator_handshake_hash),
            },
        }),
    );
}
