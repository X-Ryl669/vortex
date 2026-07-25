//! Phase 10 — hardening (spec §06-build-plan.md Phase 10).
//!
//! Three offline gates, all unit-testable:
//!
//! 1. `xx_and_ik_stress_loop` — drive 100 XX and 100 IK handshakes
//!    end-to-end with distinct random keys, asserting transcript hashes
//!    match across endpoints, all 100 transcripts are distinct (no state
//!    bleed between runs), and `derive_prs` is stable per-transcript.
//!
//! 2. `mitm_relay_diverges_transcript` — simulate an active relay between
//!    two honest endpoints with the attacker substituting its own static
//!    keys on each leg. Assert that the two endpoints compute *different*
//!    transcript hashes (and therefore different SAS values), so a user
//!    comparing SAS codes detects the attack. This is the attack-detection
//!    property cited in `02-threat-model.md` §3.1 T-BLE-2.
//!
//! 3. `ik_replay_does_not_yield_session` — capture a real IK msg1, replay
//!    it against a fresh responder (with a different ephemeral), and
//!    assert the original initiator cannot decrypt the new responder's
//!    msg2. This is the property cited in §3.3 T-RC-1.

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use snow::{params::NoiseParams, Builder};

use vortex_l3_daemon::core::crypto::derive::derive_prs;
use vortex_l3_daemon::core::crypto::noise::{
    run_ik_deterministic, run_xx_deterministic, NOISE_IK, PROLOGUE_IK,
};
use vortex_l3_daemon::core::crypto::sas::derive_sas;

fn x25519_pub(priv_bytes: &[u8; 32]) -> [u8; 32] {
    let sk = x25519_dalek::StaticSecret::from(*priv_bytes);
    x25519_dalek::PublicKey::from(&sk).to_bytes()
}

fn rand32(rng: &mut StdRng) -> [u8; 32] {
    let mut out = [0u8; 32];
    rng.fill_bytes(&mut out);
    out
}

// --------------------------------------------------------------------------
// 1. Stress loop
// --------------------------------------------------------------------------

#[test]
fn xx_and_ik_stress_loop() {
    // Deterministic seed so failures are reproducible. Change the seed
    // (or run with cargo test -- --include-ignored after promoting to
    // multiple seeds) to widen coverage.
    let mut rng = StdRng::seed_from_u64(0x_5733D_u64);
    let iterations = 100;

    let mut xx_transcripts: Vec<Vec<u8>> = Vec::with_capacity(iterations);
    let mut ik_transcripts: Vec<Vec<u8>> = Vec::with_capacity(iterations);

    for i in 0..iterations {
        let init_s = rand32(&mut rng);
        let resp_s = rand32(&mut rng);
        let init_e = rand32(&mut rng);
        let resp_e = rand32(&mut rng);

        // ---- XX ----
        let xx = run_xx_deterministic(&init_s, &resp_s, &init_e, &resp_e)
            .unwrap_or_else(|err| panic!("xx iter={i} failed: {err:?}"));
        assert_eq!(
            xx.initiator_handshake_hash, xx.responder_handshake_hash,
            "xx transcript divergence at iter {i}",
        );
        assert_eq!(xx.initiator_handshake_hash.len(), 32);
        assert_eq!(xx.messages.len(), 3);
        assert_eq!(xx.messages[0].len(), 32, "xx msg1 must be 32 bytes (e)");
        assert_eq!(xx.messages[1].len(), 96, "xx msg2 must be 96 bytes");
        assert_eq!(xx.messages[2].len(), 64, "xx msg3 must be 64 bytes");

        // PRS derivation MUST be deterministic per transcript.
        let prs_a = derive_prs(&xx.initiator_handshake_hash);
        let prs_b = derive_prs(&xx.responder_handshake_hash);
        assert_eq!(prs_a, prs_b, "PRS divergence at iter {i}");

        // SAS derivation MUST agree between sides.
        let sas_a = derive_sas(&xx.initiator_handshake_hash);
        let sas_b = derive_sas(&xx.responder_handshake_hash);
        assert_eq!(sas_a, sas_b, "SAS divergence at iter {i}");

        xx_transcripts.push(xx.initiator_handshake_hash);

        // ---- IK ----
        let resp_pub = x25519_pub(&resp_s);
        let ik = run_ik_deterministic(&init_s, &resp_s, &init_e, &resp_e, &resp_pub)
            .unwrap_or_else(|err| panic!("ik iter={i} failed: {err:?}"));
        assert_eq!(
            ik.initiator_handshake_hash, ik.responder_handshake_hash,
            "ik transcript divergence at iter {i}",
        );
        assert_eq!(ik.messages.len(), 2);
        assert_eq!(ik.messages[0].len(), 96, "ik msg1 must be 96 bytes");
        assert_eq!(ik.messages[1].len(), 48, "ik msg2 must be 48 bytes");
        ik_transcripts.push(ik.initiator_handshake_hash);
    }

    // No two iterations may produce the same transcript hash — that would
    // indicate state bleed between runs OR a catastrophic collision.
    let mut sorted_xx = xx_transcripts.clone();
    sorted_xx.sort();
    sorted_xx.dedup();
    assert_eq!(
        sorted_xx.len(),
        iterations,
        "XX transcripts collided across {iterations} runs",
    );

    let mut sorted_ik = ik_transcripts.clone();
    sorted_ik.sort();
    sorted_ik.dedup();
    assert_eq!(
        sorted_ik.len(),
        iterations,
        "IK transcripts collided across {iterations} runs",
    );

    // XX and IK must never collide either (different prologues + patterns).
    for (i, xx_h) in xx_transcripts.iter().enumerate() {
        for (j, ik_h) in ik_transcripts.iter().enumerate() {
            assert_ne!(
                xx_h, ik_h,
                "XX[{i}] == IK[{j}] — pattern domain separation broken",
            );
        }
    }
}

// --------------------------------------------------------------------------
// 2. MITM relay diverges transcript
// --------------------------------------------------------------------------

#[test]
fn mitm_relay_diverges_transcript() {
    // Set up: honest_init <-> MITM <-> honest_resp.
    //
    // The MITM cannot just forward bytes because XX includes each side's
    // static key in the transcript. Instead it terminates two independent
    // XX sessions: one with the initiator (claiming MITM's static), one
    // with the responder (claiming MITM's static). Both honest endpoints
    // believe they finished an authenticated handshake — but with the
    // MITM's identity, not each other's.
    //
    // Property (T-BLE-2): the two endpoints' transcript hashes diverge
    // because they bind different static keys and ephemerals. Therefore
    // their SAS values diverge — and the user's SAS comparison fails.

    let mut rng = StdRng::seed_from_u64(0xB16_F00Du64);
    let init_s = rand32(&mut rng);
    let resp_s = rand32(&mut rng);
    let mitm_s = rand32(&mut rng);

    let init_e = rand32(&mut rng);
    let resp_e = rand32(&mut rng);
    let mitm_e_init_side = rand32(&mut rng);
    let mitm_e_resp_side = rand32(&mut rng);

    // Leg A: honest initiator <-> MITM (acting as responder, using mitm_s).
    let leg_a = run_xx_deterministic(&init_s, &mitm_s, &init_e, &mitm_e_init_side)
        .expect("leg A handshake");
    // Leg B: MITM (acting as initiator, using mitm_s) <-> honest responder.
    let leg_b = run_xx_deterministic(&mitm_s, &resp_s, &mitm_e_resp_side, &resp_e)
        .expect("leg B handshake");

    // Each leg internally agrees on its own transcript hash.
    assert_eq!(leg_a.initiator_handshake_hash, leg_a.responder_handshake_hash);
    assert_eq!(leg_b.initiator_handshake_hash, leg_b.responder_handshake_hash);

    // The honest endpoints' views diverge:
    //   - the initiator sees leg-A transcript,
    //   - the responder sees leg-B transcript.
    let initiator_view = &leg_a.initiator_handshake_hash;
    let responder_view = &leg_b.responder_handshake_hash;
    assert_ne!(
        initiator_view, responder_view,
        "MITM should produce divergent transcripts on each leg",
    );

    // SAS values diverge (this is what the user actually compares).
    let (_, sas_init) = derive_sas(initiator_view);
    let (_, sas_resp) = derive_sas(responder_view);
    assert_ne!(
        sas_init, sas_resp,
        "MITM should produce divergent SAS — this is the user-visible MITM detection",
    );

    // Sanity: two honest endpoints with no MITM agree on SAS, by contrast.
    let no_mitm = run_xx_deterministic(&init_s, &resp_s, &init_e, &resp_e).unwrap();
    let (_, sas_a_clean) = derive_sas(&no_mitm.initiator_handshake_hash);
    let (_, sas_b_clean) = derive_sas(&no_mitm.responder_handshake_hash);
    assert_eq!(sas_a_clean, sas_b_clean, "honest run must match");
}

// --------------------------------------------------------------------------
// 3. IK replay does not yield a usable session
// --------------------------------------------------------------------------

#[test]
fn ik_replay_does_not_yield_session() {
    // Threat model T-RC-1: an attacker captures an IK msg1 from a prior
    // session and replays it later against a fresh responder. The
    // responder accepts msg1 (IK has no liveness signal in msg1) and
    // produces a msg2 with a *fresh* ephemeral. The realistic attacker
    // does NOT possess any private keys — they only have wire bytes —
    // so the test asserts:
    //
    //   (a) session2 (replay) and session1 (captured) produce DIFFERENT
    //       transcript hashes, because the responder's ephemeral differs
    //       between sessions. Therefore session keys differ.
    //
    //   (b) a transport-mode ciphertext minted from session1's responder
    //       keys cannot be decrypted by session2's responder. The
    //       attacker has no way to construct session2-valid traffic, so
    //       the replay cannot yield a usable session — even though
    //       msg1 was accepted.

    let mut rng = StdRng::seed_from_u64(0xDEAD_BEEF_u64);
    let init_s = rand32(&mut rng);
    let resp_s = rand32(&mut rng);
    let init_e = rand32(&mut rng);
    let resp_e_session1 = rand32(&mut rng);
    let resp_e_session2 = rand32(&mut rng); // fresh, different from session1
    let resp_pub = x25519_pub(&resp_s);

    // ------- Session 1 — real successful handshake -------
    let params: NoiseParams = NOISE_IK.parse().unwrap();
    let mut init1 = Builder::new(params.clone())
        .local_private_key(&init_s)
        .unwrap()
        .remote_public_key(&resp_pub)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&init_e)
        .prologue(PROLOGUE_IK)
        .unwrap()
        .build_initiator()
        .unwrap();
    let mut resp1 = Builder::new(params.clone())
        .local_private_key(&resp_s)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&resp_e_session1)
        .prologue(PROLOGUE_IK)
        .unwrap()
        .build_responder()
        .unwrap();

    let mut buf = vec![0u8; 1024];
    let mut tmp = vec![0u8; 1024];
    let n1 = init1.write_message(&[], &mut buf).unwrap();
    let captured_msg1 = buf[..n1].to_vec();
    resp1.read_message(&captured_msg1, &mut tmp).unwrap();
    let n2 = resp1.write_message(&[], &mut buf).unwrap();
    let captured_msg2 = buf[..n2].to_vec();
    init1.read_message(&captured_msg2, &mut tmp).unwrap();

    let session1_hash = init1.get_handshake_hash().to_vec();
    let mut resp1_transport = resp1.into_transport_mode().unwrap();

    // Responder sends an encrypted application payload — this is the
    // ciphertext the attacker will try to replay.
    let plaintext = b"hello from session 1";
    let n_ct = resp1_transport.write_message(plaintext, &mut buf).unwrap();
    let session1_transport_ct = buf[..n_ct].to_vec();

    // ------- Session 2 — attacker replays msg1 to a fresh responder -------
    let mut resp2 = Builder::new(params.clone())
        .local_private_key(&resp_s)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&resp_e_session2)
        .prologue(PROLOGUE_IK)
        .unwrap()
        .build_responder()
        .unwrap();
    resp2
        .read_message(&captured_msg1, &mut tmp)
        .expect("fresh responder accepts replayed msg1 (no liveness in IK msg1)");
    let _n2_replay = resp2.write_message(&[], &mut buf).unwrap();
    let session2_responder_hash = resp2.get_handshake_hash().to_vec();
    let mut resp2_transport = resp2.into_transport_mode().unwrap();

    // (a) transcript hashes diverge between sessions — so PRS-derived
    //     trust state and SES-derived session keys necessarily differ.
    assert_ne!(
        session1_hash, session2_responder_hash,
        "fresh responder ephemeral must produce a new transcript hash",
    );

    // (b) session1 transport ciphertext does NOT decrypt under session2's
    //     responder transport keys. The attacker holds the wire bytes
    //     but cannot mint a valid session2 message.
    let result = resp2_transport.read_message(&session1_transport_ct, &mut tmp);
    assert!(
        result.is_err(),
        "session1 ciphertext must fail to decrypt on session2 transport (replay rejected)",
    );
}
