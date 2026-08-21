//! Noise IK initiator + liveness probe per spec §7.

use std::time::Duration;

use futures::{pin_mut, StreamExt};
use rand::RngCore;
use snow::{params::NoiseParams, Builder, HandshakeState, TransportState};
use tokio::time::timeout;
use tracing::info;

use crate::core::ble::client::{ClientError, VortexClient};
use crate::core::ble::frame::{ty, Frame, FrameDecodeError};
use crate::core::crypto::noise::NOISE_IK;
use crate::core::crypto::x25519::X25519SecBytes;

#[derive(Debug)]
pub struct ReconnectOutcome {
    pub transcript_hash: Vec<u8>,
    pub peer_static_pub: [u8; 32],
    pub liveness_ok: bool,
    /// M6: counter value the peer reported in IK msg2 payload. Caller
    /// compares with their stored local counter — a `peer_counter`
    /// strictly less than `local_counter` is a backup-restore replay
    /// signal worth logging.
    pub peer_counter: u64,
    /// Noise transport-mode cipher pair derived from the IK handshake.
    /// Available so a caller can run an AEAD-protected post-handshake
    /// channel without re-running another IK (P2.13 BLE audio-signal
    /// path). `None` only if the caller used the legacy entrypoint that
    /// didn't ask for it — the standard path is always Some.
    pub transport: Option<TransportState>,
}

#[derive(Debug)]
pub enum ReconnectError {
    Snow(snow::Error),
    Client(ClientError),
    Timeout(&'static str),
    UnexpectedFrame { ty: u8, sub: u8 },
    FrameDecode(FrameDecodeError),
    PeerMismatch,
    NoPeerStatic,
    LivenessNonceMismatch,
}

impl std::fmt::Display for ReconnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Snow(e) => write!(f, "noise: {e}"),
            Self::Client(e) => write!(f, "ble client: {e}"),
            Self::Timeout(what) => write!(f, "timeout: {what}"),
            Self::UnexpectedFrame { ty, sub } => {
                write!(f, "unexpected frame type=0x{ty:02x} sub=0x{sub:02x}")
            }
            Self::FrameDecode(e) => write!(f, "frame decode: {e}"),
            Self::PeerMismatch => write!(f, "peer static public key did not match trusted record"),
            Self::NoPeerStatic => write!(f, "noise IK did not yield peer static public key"),
            Self::LivenessNonceMismatch => write!(f, "ping/pong nonce did not echo back"),
        }
    }
}

impl std::error::Error for ReconnectError {}

impl From<snow::Error> for ReconnectError {
    fn from(e: snow::Error) -> Self {
        Self::Snow(e)
    }
}

impl From<ClientError> for ReconnectError {
    fn from(e: ClientError) -> Self {
        Self::Client(e)
    }
}

fn build_ik_initiator(
    static_priv: &X25519SecBytes,
    peer_static_pub: &[u8; 32],
    prs: &[u8; 32],
) -> Result<HandshakeState, snow::Error> {
    let params: NoiseParams = NOISE_IK.parse()?;
    Builder::new(params)
        .local_private_key(static_priv)?
        .remote_public_key(peer_static_pub)?
        .prologue(&crate::core::crypto::noise::prologue_with_prs(prs))?
        .build_initiator()
}


/// Run Noise IK against `client`'s peer using the local static identity,
/// the trusted peer's static public key, and the Pairwise Reconnect
/// Secret (mixed into the handshake prologue).
///
/// Binding the PRS via the prologue means a long-term static-key
/// compromise alone is not enough to impersonate the trusted peer —
/// the attacker would also need the PRS, which lives only in each
/// side's secure storage.
///
/// On success, the initiator follows up with a ping/pong liveness probe
/// (frame `0x30/0x01` → `0x30/0x02`) before returning.
pub async fn run_ik_initiator(
    client: &VortexClient,
    static_priv: &X25519SecBytes,
    peer_static_pub: &[u8; 32],
    prs: &[u8; 32],
    local_counter: u64,
    wait_per_step: Duration,
) -> Result<ReconnectOutcome, ReconnectError> {
    // Subscribe to Reconnect Control notifications BEFORE sending msg1.
    let notifies = client
        .reconnect_control
        .notify()
        .await
        .map_err(ClientError::from)?;
    pin_mut!(notifies);

    let mut handshake = build_ik_initiator(static_priv, peer_static_pub, prs)?;
    let mut buffer = vec![0u8; 1024];
    let mut payload_scratch = vec![0u8; 1024];

    // ---- IK msg1 ----
    // Payload carries the local reconnect counter (M6). Encrypted by
    // Noise IK from `es` onward, so a passive observer cannot read it.
    let counter_bytes = local_counter.to_be_bytes();
    let n = handshake.write_message(&counter_bytes, &mut buffer)?;
    let frame = Frame::new(ty::RECONNECT_HANDSHAKE, 0x01, buffer[..n].to_vec());
    client.write_reconnect_control(&frame).await?;
    info!("→ IK msg1 sent ({} bytes, counter={local_counter})", n);

    // ---- IK msg2 ----
    let raw = timeout(wait_per_step, notifies.next())
        .await
        .map_err(|_| ReconnectError::Timeout("msg2 notify"))?
        .ok_or(ReconnectError::Timeout("notify stream closed"))?;
    let msg2 = Frame::decode(&raw).map_err(ReconnectError::FrameDecode)?;
    if msg2.ty != ty::RECONNECT_HANDSHAKE || msg2.sub != 0x02 {
        return Err(ReconnectError::UnexpectedFrame {
            ty: msg2.ty,
            sub: msg2.sub,
        });
    }
    let pt_len = handshake.read_message(&msg2.payload, &mut payload_scratch)?;
    let peer_counter: u64 = if pt_len >= 8 {
        u64::from_be_bytes(payload_scratch[..8].try_into().unwrap())
    } else {
        0
    };
    info!(
        "← IK msg2 received ({} bytes, peer_counter={peer_counter})",
        msg2.payload.len()
    );

    // Verify peer's static matches the trusted record.
    let peer_pub_observed = handshake
        .get_remote_static()
        .ok_or(ReconnectError::NoPeerStatic)?;
    if peer_pub_observed != peer_static_pub {
        return Err(ReconnectError::PeerMismatch);
    }
    let transcript_hash = handshake.get_handshake_hash().to_vec();
    // Promote the IK handshake to transport mode BEFORE doing the
    // liveness probe — the ping/pong is plain ATT, but capturing the
    // ciphers here means we don't need a second IK over BLE for the
    // P2.13 audio-signal channel. The handshake is consumed so no
    // separate `drop(handshake)` is needed.
    let transport = handshake.into_transport_mode()?;

    // ---- Liveness probe (ping → pong) ----
    let mut nonce = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ping = Frame::new(ty::TRANSPORT_KEEPALIVE, 0x01, nonce.to_vec());
    client.write_reconnect_control(&ping).await?;
    info!("→ ping ({})", hex::encode(nonce));

    let raw = timeout(wait_per_step, notifies.next())
        .await
        .map_err(|_| ReconnectError::Timeout("pong"))?
        .ok_or(ReconnectError::Timeout("notify stream closed"))?;
    let pong = Frame::decode(&raw).map_err(ReconnectError::FrameDecode)?;
    if pong.ty != ty::TRANSPORT_KEEPALIVE || pong.sub != 0x02 {
        return Err(ReconnectError::UnexpectedFrame {
            ty: pong.ty,
            sub: pong.sub,
        });
    }
    if pong.payload.as_slice() != nonce {
        return Err(ReconnectError::LivenessNonceMismatch);
    }
    info!("← pong matched");

    Ok(ReconnectOutcome {
        transcript_hash,
        peer_static_pub: *peer_static_pub,
        liveness_ok: true,
        peer_counter,
        transport: Some(transport),
    })
}
