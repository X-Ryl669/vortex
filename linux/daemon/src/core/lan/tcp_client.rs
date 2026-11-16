//! Noise IK initiator + liveness probe over TCP per spec §8.3 and §9.2.

use std::net::SocketAddr;
use std::time::Duration;

use rand::RngCore;
use snow::{params::NoiseParams, Builder, HandshakeState};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::info;

use crate::core::ble::frame::{ty, Frame, FrameDecodeError, FRAME_HEADER_LEN, MAX_FRAME_PAYLOAD};
use crate::core::crypto::noise::NOISE_IK;
use crate::core::crypto::x25519::X25519SecBytes;

#[derive(Debug, Clone)]
pub struct LanReconnectOutcome {
    pub transcript_hash: Vec<u8>,
    pub peer_static_pub: [u8; 32],
    pub liveness_ok: bool,
    pub remote: SocketAddr,
    /// M6: peer's reported counter (see `ReconnectOutcome`).
    pub peer_counter: u64,
    /// Peer-reported app state (battery, device class, locale, theme,
    /// earbuds). Populated after the post-handshake app-data exchange.
    pub peer_state: Option<crate::core::appstate::AppState>,
    /// How long ago [`peer_state`] was actually read off the wire.
    ///
    /// It is exchanged EARLY in the round and the caller only sees it at the
    /// END, after bulk-sync — which can sit for its full idle timeout when the
    /// phone is busy. A snapshot is a level, not an event, so applying a stale
    /// one to the UI undoes newer truth: live-caught a 15s-old `ringing`
    /// replacing the in-call pill of a call that had been answered seven
    /// seconds earlier, which also turned it into a "missed call". The caller
    /// cannot tell without this, because nothing else in the outcome says when
    /// any of it happened.
    pub peer_state_age: std::time::Duration,
    /// Bulk-sync datasets the phone shipped because our cached hash was
    /// stale: (frame type, reassembled JSON bytes). Empty when everything
    /// matched, no request was made, or the peer predates BULK_SYNC.
    pub bulk: Vec<(u8, Vec<u8>)>,
    /// Per-dataset outcome from the bulk-sync DONE frame, e.g.
    /// `{"contacts":"match","clipboard_file":"nomatch"}`. `None` when no
    /// request was made or the peer never sent a done frame (predates
    /// BULK_SYNC, or the exchange broke off).
    ///
    /// Datasets report their own failures HERE and nowhere else: a "nomatch"
    /// looks exactly like "nothing to send" in [`bulk`], so a caller holding a
    /// pull request open (the instant-share file queue) needs this to learn
    /// that what it asked for is never coming.
    pub bulk_status: Option<BulkStatus>,
    /// File offers the phone announced on this round, from the done frame's
    /// `offers` array.
    ///
    /// The same announcement BLE carries, on the transport that is actually
    /// up. A capture is offered over BLE the moment it is taken, and with BLE
    /// down that offer is retried for a minute and then dropped — while this
    /// exchange completed cleanly every twelve seconds throughout. The phone
    /// re-announces whatever is still pending here, so the link that works is
    /// the one that delivers.
    pub offers: Vec<crate::core::clipboard_mirror::ClipboardImageOffer>,
    /// Capture tokens the phone no longer has the original of, from the done
    /// frame's `deleted` array.
    ///
    /// A picture deleted on the phone should not leave its automatic copy on
    /// the laptop for ever. Only the phone reports this direction: our copy is
    /// ours to remove, while deleting the phone's own original would need a
    /// system consent dialog there for every file.
    pub deleted: Vec<String>,
    /// Clipboard text the phone copied but could not deliver over BLE, from
    /// the done frame's `clipboard` object.
    ///
    /// Phone→laptop clipboard text has no transport but the BLE notify, so a
    /// laptop whose GATT link never connects silently loses every copy while
    /// this exchange succeeds every few seconds. Carried here so the link that
    /// works is the one that delivers, exactly as for [`offers`].
    pub clipboard: Option<crate::core::clipboard_mirror::ClipboardMirror>,
}

/// Pull the done frame's `offers` array out as real offers.
///
/// Separate from [`BulkStatus`], which keeps only string values and would drop
/// an array without a word. Malformed entries are skipped one by one: a phone
/// announcing three captures must not lose all three because one of them
/// carried a field this build cannot read.
fn parse_offers(json: &[u8]) -> Vec<crate::core::clipboard_mirror::ClipboardImageOffer> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = v.get("offers").and_then(|o| o.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| serde_json::from_value(e.clone()).ok())
        .collect()
}

/// Pull the done frame's `deleted` array out as capture tokens.
///
/// Same shape and same forgiveness as [`parse_offers`]: a non-string entry is
/// skipped rather than costing the rest, because one unreadable token must not
/// strand every other deletion on the laptop.
fn parse_deleted(json: &[u8]) -> Vec<String> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = v.get("deleted").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    arr.iter().filter_map(|e| e.as_str().map(|s| s.to_string())).collect()
}

/// Pull the done frame's `clipboard` object out as text the phone copied.
///
/// The phone's clipboard text is normally a BLE notify. When the GATT link is
/// down that notify fails and the text is gone, so the phone hands it to this
/// exchange instead — the same escape hatch `offers` uses, for the one payload
/// that had no fallback at all.
fn parse_clipboard(json: &[u8]) -> Option<crate::core::clipboard_mirror::ClipboardMirror> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let clip = v.get("clipboard")?;
    let parsed: crate::core::clipboard_mirror::ClipboardMirror =
        serde_json::from_value(clip.clone()).ok()?;
    if parsed.text.is_empty() {
        return None;
    }
    Some(parsed)
}

/// The done frame with any clipboard body replaced by its length.
///
/// The frame is logged verbatim, and clipboard content must never reach the
/// log (the BLE path logs `chars = …` for exactly this reason). Everything
/// else in the frame is dataset names and tokens, which are worth keeping.
fn redact_done_frame(json: &[u8]) -> String {
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(json) else {
        return String::from_utf8_lossy(json).into_owned();
    };
    if let Some(obj) = v.as_object_mut() {
        if let Some(clip) = obj.get_mut("clipboard") {
            let chars = clip
                .get("text")
                .and_then(|t| t.as_str())
                .map(|t| t.chars().count())
                .unwrap_or(0);
            *clip = serde_json::json!({ "text": format!("<{chars} chars>") });
        }
    }
    v.to_string()
}

/// The bulk-sync done frame's per-dataset outcome map.
#[derive(Debug, Clone, Default)]
pub struct BulkStatus(std::collections::HashMap<String, String>);

impl BulkStatus {
    /// Parse the done frame's JSON body. Non-string values are ignored rather
    /// than failing the whole map — one odd field must not blind the caller to
    /// the rest.
    fn parse(json: &[u8]) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_slice(json).ok()?;
        let obj = v.as_object()?;
        Some(Self(
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect(),
        ))
    }

    /// What the phone reported for `dataset` ("sent" / "match" / "nomatch" /
    /// "error" / "unknown"), or `None` if it said nothing about it.
    pub fn get(&self, dataset: &str) -> Option<&str> {
        self.0.get(dataset).map(String::as_str)
    }

    /// True when the phone answered about `dataset` and it was NOT served —
    /// i.e. asking again is pointless until something changes on its side.
    pub fn unservable(&self, dataset: &str) -> bool {
        matches!(self.get(dataset), Some(s) if s != "sent")
    }
}

#[derive(Debug)]
pub enum LanError {
    Snow(snow::Error),
    Io(std::io::Error),
    Timeout(&'static str),
    UnexpectedFrame { ty: u8, sub: u8 },
    FrameDecode(FrameDecodeError),
    PeerMismatch,
    NoPeerStatic,
    LivenessNonceMismatch,
    OversizeFrame(usize),
}

impl std::fmt::Display for LanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Snow(e) => write!(f, "noise: {e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Timeout(what) => write!(f, "timeout: {what}"),
            Self::UnexpectedFrame { ty, sub } => {
                write!(f, "unexpected frame type=0x{ty:02x} sub=0x{sub:02x}")
            }
            Self::FrameDecode(e) => write!(f, "frame decode: {e}"),
            Self::PeerMismatch => write!(f, "peer static did not match trusted record"),
            Self::NoPeerStatic => write!(f, "noise IK did not yield peer static public key"),
            Self::LivenessNonceMismatch => write!(f, "ping/pong nonce did not echo back"),
            Self::OversizeFrame(n) => write!(f, "incoming frame declared length {n} > max"),
        }
    }
}

impl std::error::Error for LanError {}

impl From<snow::Error> for LanError {
    fn from(e: snow::Error) -> Self {
        Self::Snow(e)
    }
}
impl From<std::io::Error> for LanError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub(crate) fn build_ik_initiator(
    static_priv: &X25519SecBytes,
    peer_static_pub: &[u8; 32],
    prs: &[u8; 32],
) -> Result<HandshakeState, snow::Error> {
    // PRS is mixed into the prologue rather than as a Noise PSK,
    // because the Noise-Java library on Android does not implement the
    // standardized `psk<n>` modifier syntax. Mixing into the prologue
    // achieves the same goal — wrong PRS yields different MixHash and
    // breaks AEAD verification on msg1.
    let params: NoiseParams = NOISE_IK.parse()?;
    let prologue = crate::core::crypto::noise::prologue_with_prs(prs);
    Builder::new(params)
        .local_private_key(static_priv)?
        .remote_public_key(peer_static_pub)?
        .prologue(&prologue)?
        .build_initiator()
}

async fn write_frame(stream: &mut TcpStream, frame: &Frame) -> Result<(), LanError> {
    let bytes = frame.encode();
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

/// Bound applied to a single frame read. Defaults to the protocol
/// max (`MAX_FRAME_PAYLOAD`), but the caller can pass a tighter cap
/// for steps whose expected payload is known ahead of time (e.g.
/// IK msg2 is ~48 bytes; capping there means an attacker can't
/// declare 8KB and force an oversized allocation).
async fn read_frame_with_cap(
    stream: &mut TcpStream,
    max_payload: usize,
) -> Result<Frame, LanError> {
    let cap = max_payload.min(MAX_FRAME_PAYLOAD);
    let mut header = [0u8; FRAME_HEADER_LEN];
    stream.read_exact(&mut header).await?;
    let length = u16::from_be_bytes([header[2], header[3]]) as usize;
    if length > cap {
        return Err(LanError::OversizeFrame(length));
    }
    let mut full = vec![0u8; FRAME_HEADER_LEN + length];
    full[..FRAME_HEADER_LEN].copy_from_slice(&header);
    if length > 0 {
        stream.read_exact(&mut full[FRAME_HEADER_LEN..]).await?;
    }
    Frame::decode(&full).map_err(LanError::FrameDecode)
}

async fn read_frame(stream: &mut TcpStream) -> Result<Frame, LanError> {
    read_frame_with_cap(stream, MAX_FRAME_PAYLOAD).await
}

/// Connect to `addr` over TCP and run Noise IK + ping/pong. PRS is
/// mixed into the prologue so reconnect requires both the long-term
/// static key AND the pairwise reconnect secret.
pub async fn run_lan_reconnect(
    addr: SocketAddr,
    static_priv: &X25519SecBytes,
    peer_static_pub: &[u8; 32],
    prs: &[u8; 32],
    local_counter: u64,
    local_state: crate::core::appstate::AppState,
    wait_per_step: Duration,
    // BULK_SYNC request JSON (`{"contacts":"<sha256-hex of our cache>"}`),
    // sent after the app-state exchange. None skips the stage (CLI).
    bulk_request: Option<&str>,
) -> Result<LanReconnectOutcome, LanError> {
    info!(%addr, "TCP connecting");
    let mut stream = timeout(wait_per_step, TcpStream::connect(addr))
        .await
        .map_err(|_| LanError::Timeout("tcp connect"))??;
    // Disable Nagle — file chunks are large back-to-back frames; Nagle's
    // wait-for-ACK throttled the pull to a crawl. NODELAY ⇒ full LAN speed.
    let _ = stream.set_nodelay(true);
    info!(%addr, "TCP connected");

    let mut handshake = build_ik_initiator(static_priv, peer_static_pub, prs)?;
    let mut buf = vec![0u8; 1024];
    let mut tmp = vec![0u8; 1024];

    // ---- IK msg1 ----
    // Payload carries the local reconnect counter (M6).
    let counter_bytes = local_counter.to_be_bytes();
    let n = handshake.write_message(&counter_bytes, &mut buf)?;
    let msg1 = Frame::new(ty::RECONNECT_HANDSHAKE, 0x01, buf[..n].to_vec());
    write_frame(&mut stream, &msg1).await?;
    info!("→ IK msg1 over TCP ({} bytes, counter={local_counter})", n);

    // ---- IK msg2 ----
    // IK msg2 ≈ 32 + 16 = 48 bytes + 8-byte counter payload + 16-byte
    // tag = ~72 bytes. Cap at 128 so a hostile responder can't declare
    // a max-size allocation.
    let msg2 = timeout(wait_per_step, read_frame_with_cap(&mut stream, 128))
        .await
        .map_err(|_| LanError::Timeout("msg2"))??;
    if msg2.ty != ty::RECONNECT_HANDSHAKE || msg2.sub != 0x02 {
        return Err(LanError::UnexpectedFrame {
            ty: msg2.ty,
            sub: msg2.sub,
        });
    }
    let pt_len = handshake.read_message(&msg2.payload, &mut tmp)?;
    let peer_counter: u64 = if pt_len >= 8 {
        u64::from_be_bytes(tmp[..8].try_into().unwrap())
    } else {
        0
    };
    info!(
        "← IK msg2 over TCP ({} bytes, peer_counter={peer_counter})",
        msg2.payload.len()
    );

    let observed_pub = handshake
        .get_remote_static()
        .ok_or(LanError::NoPeerStatic)?;
    if observed_pub != peer_static_pub {
        return Err(LanError::PeerMismatch);
    }
    let transcript_hash = handshake.get_handshake_hash().to_vec();
    // Promote to Noise transport mode so we can AEAD-wrap all
    // post-handshake frames below. Must happen before `handshake` is
    // dropped — `into_transport_mode` consumes it.
    //
    // **Nonce-ordering invariant:** the `transport` cipher pair has a
    // single monotonically-increasing nonce per direction. Every
    // post-handshake `write_message` / `read_message` consumes one
    // nonce. The on-the-wire frame sequence is fixed:
    //   1. ping (write)  → pong (read)
    //   2. app-state out (write inside exchange_app_state)
    //      app-state in  (read  inside exchange_app_state)
    // Both peers MUST step through this sequence in the same order.
    // Adding a new frame type means appending it to the end (not
    // interleaving) — otherwise the nonces desynchronize and AEAD
    // decrypt fails on the next frame. The mirror Kotlin code in
    // `a3/.../LanServer.kt` follows the same ordering.
    let mut transport = handshake.into_transport_mode()?;

    // ---- ping / pong ----
    let mut nonce = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ping = Frame::new(ty::TRANSPORT_KEEPALIVE, 0x01, nonce.to_vec());
    write_frame(&mut stream, &ping).await?;
    info!("→ ping ({})", hex::encode(nonce));

    let pong = timeout(wait_per_step, read_frame(&mut stream))
        .await
        .map_err(|_| LanError::Timeout("pong"))??;
    if pong.ty != ty::TRANSPORT_KEEPALIVE || pong.sub != 0x02 {
        return Err(LanError::UnexpectedFrame {
            ty: pong.ty,
            sub: pong.sub,
        });
    }
    if pong.payload.as_slice() != nonce {
        return Err(LanError::LivenessNonceMismatch);
    }
    info!("← pong matched");

    // ---- App-state sync (battery, device class, locale, theme, earbuds) ----
    // Pairing security is bounded by IK + ping/pong above. Each transport
    // runs its own IK per spec §8.5 — V1 does NOT use a channel join
    // proof (deferred to V2). App-level state lives in a separate
    // post-handshake exchange so we can evolve it without touching
    // pairing code.
    let peer_state_read_at = std::time::Instant::now();
    let peer_state = match crate::core::appstate::exchange_app_state(
        &mut stream,
        &mut transport,
        &local_state,
        wait_per_step,
    )
    .await
    {
        Ok(s) => {
            info!(
                "↔ app-state synced (peer battery={:?} class={:?})",
                s.battery, s.class
            );
            Some(s)
        }
        Err(e) => {
            tracing::warn!("app-state exchange failed: {e}");
            None
        }
    };

    // ---- Bulk-sync (BULK_SYNC 0x3D) ----
    // Hash-gated mirror transfer: we name our cached datasets' hashes, the
    // phone ships full JSON only for stale ones — over THIS reliable TCP
    // socket instead of a BLE notify burst. Skipped when the app-state
    // exchange already failed (transport unhealthy) or no request was made.
    let mut bulk: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut bulk_status: Option<BulkStatus> = None;
    let mut offers: Vec<crate::core::clipboard_mirror::ClipboardImageOffer> = Vec::new();
    let mut deleted: Vec<String> = Vec::new();
    let mut clipboard: Option<crate::core::clipboard_mirror::ClipboardMirror> = None;
    if let (Some(req), Some(_)) = (bulk_request, peer_state.as_ref()) {
        match exchange_bulk(&mut stream, &mut transport, req, wait_per_step).await {
            Ok(ex) => {
                bulk = ex.datasets;
                bulk_status = ex.status;
                offers = ex.offers;
                deleted = ex.deleted;
                clipboard = ex.clipboard;
            }
            Err(e) => tracing::warn!("bulk-sync exchange failed: {e}"),
        }
    }

    // Outgoing file share (laptop → phone): offer a queued batch now over this
    // established session, after bulk-sync, so the phone's control loop reads
    // it in nonce lock-step. FIFO-drained across heartbeats.
    // The phone pulls each file over this session; the push reports its own
    // terminal state to the pill, so an error here only needs logging.
    if let Some(files) = crate::core::outgoing_share::take_batch() {
        if let Err(e) = push_outgoing_batch(&mut stream, &mut transport, &files).await {
            tracing::warn!("file push failed: {e}");
        }
    }

    let _ = stream.shutdown().await;
    Ok(LanReconnectOutcome {
        transcript_hash,
        peer_static_pub: *peer_static_pub,
        liveness_ok: true,
        remote: addr,
        peer_counter,
        peer_state,
        peer_state_age: peer_state_read_at.elapsed(),
        bulk,
        bulk_status,
        offers,
        deleted,
        clipboard,
    })
}

/// Seal one frame with the transport cipher and write it. The nonce advances
/// once per call — callers MUST stay in lock-step with the peer's reads.
async fn send_sealed(
    stream: &mut TcpStream,
    transport: &mut snow::TransportState,
    ty_byte: u8,
    plain: &[u8],
) -> Result<(), LanError> {
    let mut ct = vec![0u8; plain.len() + 16];
    let n = transport.write_message(plain, &mut ct)?;
    ct.truncate(n);
    let frame = Frame::new(ty_byte, 0, ct);
    stream.write_all(&frame.encode()).await?;
    stream.flush().await?;
    Ok(())
}

/// Read one transport frame within `wait` (the ciphertext is decrypted by the
/// caller). Used for the phone's FILE_PUSH_DECISION reply.
async fn recv_frame(stream: &mut TcpStream, wait: Duration) -> Result<Frame, LanError> {
    timeout(wait, read_frame(stream))
        .await
        .map_err(|_| LanError::Timeout("file-push decision"))?
}

/// How a pushed batch ended, short of a transport error.
enum PushEnd {
    /// The phone user declined (or let the prompt time out).
    Declined,
    /// Delivered; names the files the phone reported it could not save.
    Done(Vec<String>),
    /// Not sent, for a reason the user should be told.
    Refused(String),
}

/// Push a batch of laptop-shared files to the phone (instant-share, reverse).
///
/// Every file is granted a `share:<token>` address ([`crate::core::share_grants`])
/// and one FILE_PUSH_OFFER lists the batch; the phone shows a single consent
/// prompt and replies FILE_PUSH_DECISION. On accept the phone PULLS each file
/// through the ranged-filesystem protocol on this same session, writing every
/// range straight to its Downloads, and closes with a FILE_PUSH_DONE report.
/// Nothing is held in memory on either side, so there is no size limit.
///
/// Reports its own terminal progress (Declined / Done / Fail). Grants are
/// revoked and temporary archives deleted however the batch ends.
async fn push_outgoing_batch(
    stream: &mut TcpStream,
    transport: &mut snow::TransportState,
    files: &[crate::core::outgoing_share::OutgoingFile],
) -> Result<(), LanError> {
    use crate::core::outgoing_share::{discard, report_progress, InFlight, OutProgress};
    use crate::core::share_grants;

    let _busy = InFlight::begin();
    let tokens: Vec<String> = files
        .iter()
        .map(|f| share_grants::grant(f.path.clone()))
        .collect();
    let result = push_granted(stream, transport, files, &tokens).await;
    for t in &tokens {
        share_grants::revoke(t);
    }
    discard(files);

    match result {
        Ok(PushEnd::Declined) => {
            info!("← file push declined by phone ({} files)", files.len());
            report_progress(OutProgress::Declined);
            Ok(())
        }
        Ok(PushEnd::Done(failed)) => {
            report_progress(OutProgress::Done { failed });
            Ok(())
        }
        Ok(PushEnd::Refused(reason)) => {
            tracing::warn!("file push not sent: {reason}");
            report_progress(OutProgress::Fail { reason });
            Ok(())
        }
        Err(e) => {
            report_progress(OutProgress::Fail {
                reason: push_failure_reason(&e),
            });
            Err(e)
        }
    }
}

/// A transport error, in words for the notification.
fn push_failure_reason(e: &LanError) -> String {
    match e {
        LanError::Timeout(_) => "the phone stopped responding".to_string(),
        LanError::Io(_) => "the connection to the phone dropped".to_string(),
        _ => format!("transfer error ({e})"),
    }
}

async fn push_granted(
    stream: &mut TcpStream,
    transport: &mut snow::TransportState,
    files: &[crate::core::outgoing_share::OutgoingFile],
    tokens: &[String],
) -> Result<PushEnd, LanError> {
    use crate::core::outgoing_share::{decision, report_progress, OutProgress};
    use crate::core::share_grants::SHARE_PREFIX;

    let count = files.len() as u32;
    let total_bytes: u64 = files.iter().map(|f| f.size).sum();
    let label = crate::core::outgoing_share::batch_label(files);
    report_progress(OutProgress::Start {
        label,
        count,
        total: total_bytes,
    });

    // Offer: the full file list + aggregate total, so the phone can render one
    // consent prompt ("N files · X MB"). `path` is what the phone OPENs to pull
    // the file; `pull` tells it the laptop can serve that.
    let manifest: Vec<serde_json::Value> = files
        .iter()
        .zip(tokens)
        .map(|(f, t)| {
            let mut o = serde_json::json!({
                "name": f.name,
                "mime": f.mime,
                "bytes": f.size,
                "path": format!("{SHARE_PREFIX}{t}"),
                "extract": f.folder.is_some(),
            });
            if let Some(z) = &f.folder {
                o["unpacked"] = z.unpacked.into();
                o["entries"] = z.entries.into();
            }
            o
        })
        .collect();
    let offer = serde_json::json!({
        "files": manifest, "count": count, "total": total_bytes, "pull": true,
    })
    .to_string();
    // The offer is a single frame. Saying so beats sending one the phone will
    // drop as oversize, which would read as the prompt never appearing.
    if offer.len() + 16 > MAX_FRAME_PAYLOAD {
        return Ok(PushEnd::Refused(format!(
            "too many files in one share ({count}) — zip them, or share a folder"
        )));
    }
    send_sealed(stream, transport, ty::FILE_PUSH_OFFER, offer.as_bytes()).await?;

    // Consent: wait for the phone user's Accept/Decline (the phone caps its own
    // prompt at ~45 s; give it 60 s before we treat silence as a decline).
    let decision_frame = recv_frame(stream, Duration::from_secs(60)).await?;
    if decision_frame.ty != ty::FILE_PUSH_DECISION {
        return Err(LanError::UnexpectedFrame {
            ty: decision_frame.ty,
            sub: decision_frame.sub,
        });
    }
    let mut pt = vec![0u8; decision_frame.payload.len()];
    let n = transport.read_message(&decision_frame.payload, &mut pt)?;
    pt.truncate(n);
    match pt.first().copied() {
        Some(decision::ACCEPT_PULL) => {
            report_progress(OutProgress::Accepted);
            serve_pull(stream, transport, files, tokens, total_bytes).await
        }
        Some(decision::ACCEPT_STREAM) => {
            report_progress(OutProgress::Accepted);
            stream_legacy(stream, transport, files, total_bytes).await
        }
        _ => Ok(PushEnd::Declined),
    }
}

/// Serve the phone's pull of an accepted batch, on this session, until it
/// reports FILE_PUSH_DONE.
///
/// The phone reads each `share:` path in ranges with the same FS_REQ ops it
/// uses to browse, so this is the ordinary filesystem server over this socket
/// — `fs_server` already answers `share:` opens from the grant table. Its
/// browse roots are served too: the phone sends ALL its filesystem requests on
/// the session that last carried one, so a browse started mid-transfer lands
/// here and must not be refused for arriving on the wrong socket.
///
/// One request at a time, in arrival order: the phone pipelines a few reads,
/// TCP queues them, and answering in order is what keeps the Noise nonces in
/// lock-step on both sides.
async fn serve_pull(
    stream: &mut TcpStream,
    transport: &mut snow::TransportState,
    files: &[crate::core::outgoing_share::OutgoingFile],
    tokens: &[String],
    total_bytes: u64,
) -> Result<PushEnd, LanError> {
    use crate::core::ble::frame::sub;
    use crate::core::fs_proto as p;
    use crate::core::fs_server::{self, FsHandles, Served};
    use crate::core::outgoing_share::{report_progress, OutProgress, PushReport};
    use crate::core::share_grants::token_of;
    use std::collections::HashMap;
    use std::sync::Arc;

    // Sliding, not total: a large file on a slow link keeps making progress and
    // must never be cut off, while a phone that stops asking (killed, out of
    // range) still releases the session. Longer than the phone's own 20 s
    // request timeout, so it is always the phone that gives up first and can
    // say why.
    const IDLE: Duration = Duration::from_secs(30);

    let roots = Arc::new(fs_server::load_roots());
    let handles = Arc::new(FsHandles::new());
    // Progress bookkeeping: which batch file each handle reads, and how far
    // into each file the phone has got.
    let mut file_of_handle: HashMap<u64, usize> = HashMap::new();
    let mut reached: Vec<u64> = vec![0; files.len()];

    loop {
        let frame = timeout(IDLE, read_frame(stream))
            .await
            .map_err(|_| LanError::Timeout("file pull (phone went quiet)"))??;
        let mut pt = vec![0u8; frame.payload.len()];
        let n = transport.read_message(&frame.payload, &mut pt)?;
        pt.truncate(n);

        match frame.ty {
            ty::FS_REQ => {
                let op = frame.sub;
                let opening = (op == p::op::OPEN)
                    .then(|| serde_json::from_slice::<p::OpenReq>(&pt).ok())
                    .flatten()
                    .and_then(|r| {
                        let t = token_of(&r.path)?;
                        tokens.iter().position(|x| x == t)
                    });
                let reading = (op == p::op::READ)
                    .then(|| serde_json::from_slice::<p::ReadReq>(&pt).ok())
                    .flatten()
                    .and_then(|r| file_of_handle.get(&r.handle).copied());

                // `fs_server` is synchronous and touches the disk.
                let served = {
                    let (roots, handles) = (roots.clone(), handles.clone());
                    tokio::task::spawn_blocking(move || fs_server::serve(&roots, &handles, op, &pt))
                        .await
                        .map_err(|e| LanError::Io(std::io::Error::other(e)))?
                };
                let (ty_byte, payload) = match served {
                    Served::Meta(reply) => {
                        if let (Some(i), p::FsReply::Open { handle, .. }) = (opening, &reply) {
                            file_of_handle.insert(*handle, i);
                        }
                        (ty::FS_META, serde_json::to_vec(&reply).unwrap_or_default())
                    }
                    Served::Data(bytes) => {
                        if let Some(i) = reading {
                            if let Some((_, off, _, b)) = p::decode_data(&bytes) {
                                reached[i] = reached[i].max(off + b.len() as u64);
                                report_progress(OutProgress::Progress {
                                    sent: reached.iter().sum(),
                                    total: total_bytes,
                                });
                            }
                        }
                        (ty::FS_DATA, bytes)
                    }
                    Served::Err(e) => (ty::FS_ERR, serde_json::to_vec(&e).unwrap_or_default()),
                };
                send_sealed(stream, transport, ty_byte, &payload).await?;
            }
            ty::FILE_PUSH if frame.sub == sub::FILE_PUSH_DONE => {
                let report: PushReport = serde_json::from_slice(&pt).unwrap_or_default();
                info!(
                    "→ file push batch pulled ({} saved, {} failed, {} bytes)",
                    report.saved,
                    report.failed.len(),
                    reached.iter().sum::<u64>()
                );
                return Ok(PushEnd::Done(report.failed));
            }
            other => {
                return Err(LanError::UnexpectedFrame {
                    ty: other,
                    sub: frame.sub,
                })
            }
        }
    }
}

/// The pre-pull wire, for a phone app that answered ACCEPT_STREAM: each file
/// as FILE_PUSH `[total][idx][data]` chunks, in order. Read from disk a chunk at
/// a time, so this side still holds nothing; the old phone app assembles each
/// file in its memory, which is the limit the pull exists to remove.
async fn stream_legacy(
    stream: &mut TcpStream,
    transport: &mut snow::TransportState,
    files: &[crate::core::outgoing_share::OutgoingFile],
    total_bytes: u64,
) -> Result<PushEnd, LanError> {
    use crate::core::outgoing_share::{
        legacy_chunk, legacy_chunk_count, report_progress, OutProgress, PUSH_CHUNK_BYTES,
    };
    use tokio::io::AsyncReadExt as _;

    // Checked up front, for the whole batch: once the phone has accepted it
    // reads files strictly in order, so one that cannot be sent cannot simply
    // be skipped mid-stream.
    if let Some(f) = files.iter().find(|f| legacy_chunk_count(f.size).is_none()) {
        return Ok(PushEnd::Refused(format!(
            "'{}' is too large for the Vortex app on the phone — update the phone app",
            f.name
        )));
    }

    let count = files.len();
    let mut sent: u64 = 0;
    let mut buf = vec![0u8; PUSH_CHUNK_BYTES];
    for (fi, f) in files.iter().enumerate() {
        let mut file = tokio::fs::File::open(&f.path).await?;
        // Chunk count is fixed by the size we offered: the phone keys its
        // assembly off the total in the first chunk. A file that changed size
        // since then is sent truncated or zero-padded to that, not desynced.
        let total = legacy_chunk_count(f.size).unwrap_or(1);
        let mut left = f.size;
        for idx in 0..total {
            let want = (left as usize).min(PUSH_CHUNK_BYTES);
            let mut got = 0;
            while got < want {
                match file.read(&mut buf[got..want]).await? {
                    0 => break,
                    n => got += n,
                }
            }
            buf[got..want].fill(0);
            send_sealed(stream, transport, ty::FILE_PUSH, &legacy_chunk(total, idx, &buf[..want]))
                .await?;
            left -= want as u64;
            sent += want as u64;
            report_progress(OutProgress::Progress {
                sent,
                total: total_bytes,
            });
        }
        info!("→ file push [{}/{count}] '{}' ({} bytes, legacy stream)", fi + 1, f.name, f.size);
    }
    info!("→ file push batch done ({count} files, {total_bytes} bytes)");
    Ok(PushEnd::Done(Vec::new()))
}

/// What one bulk-sync round brought back.
struct BulkExchange {
    /// Datasets the phone shipped because our cached hash was stale.
    datasets: Vec<(u8, Vec<u8>)>,
    /// The done frame's per-dataset outcome map.
    status: Option<BulkStatus>,
    /// File offers the phone re-announced on this round.
    offers: Vec<crate::core::clipboard_mirror::ClipboardImageOffer>,
    /// Capture tokens whose originals are gone from the phone.
    deleted: Vec<String>,
    /// Clipboard text the phone could not push over BLE, if any.
    clipboard: Option<crate::core::clipboard_mirror::ClipboardMirror>,
}

/// Send the bulk-sync request and collect the chunked dataset frames the
/// phone ships back for stale hashes, until its done frame (sub 0x02) or
/// the time budget runs out. An old phone build that doesn't know
/// BULK_SYNC simply never answers — the timeout path degrades gracefully.
async fn exchange_bulk(
    stream: &mut TcpStream,
    transport: &mut snow::TransportState,
    request_json: &str,
    wait: Duration,
) -> Result<BulkExchange, LanError> {
    let plain = request_json.as_bytes();
    let mut ct = vec![0u8; plain.len() + 16];
    let n = transport.write_message(plain, &mut ct)?;
    ct.truncate(n);
    let frame = Frame::new(ty::BULK_SYNC, 0x01, ct);
    stream.write_all(&frame.encode()).await?;
    stream.flush().await?;
    info!("→ bulk-sync request ({} bytes)", plain.len());

    let mut out: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut status: Option<BulkStatus> = None;
    let mut offers: Vec<crate::core::clipboard_mirror::ClipboardImageOffer> = Vec::new();
    let mut clipboard: Option<crate::core::clipboard_mirror::ClipboardMirror> = None;
    let mut deleted: Vec<String> = Vec::new();
    let mut listing = crate::core::phone_files::ListingAssembler::default();
    let mut contacts = crate::core::contacts::ContactsAssembler::default();
    let mut call_log = crate::core::call_log::CallLogAssembler::default();
    let mut sms = crate::core::sms::SmsAssembler::default();
    let mut sms_history = crate::core::sms::SmsAssembler::default();
    let mut call_log_history = crate::core::call_log::CallLogAssembler::default();
    let mut sms_ids = crate::core::sms::SmsAssembler::default();
    let mut clipboard_image = crate::core::clipboard_mirror::ImageAssembler::default();
    let mut clipboard_file = crate::core::clipboard_mirror::ImageAssembler::default();
    let mut file_chunks_seen: u32 = 0; // for incoming-file progress reporting
    let mut file_start: Option<tokio::time::Instant> = None; // for throughput log
    // SLIDING idle budget: reset on every frame, so a large file pull that keeps
    // making progress never gets cut off (the old fixed 60 s cap made a >~40 MB
    // file on a slow link time out → re-request from scratch → loop forever).
    // A genuinely stalled transfer still aborts after IDLE with no frame.
    const IDLE: Duration = Duration::from_secs(30);
    let mut deadline = tokio::time::Instant::now() + IDLE;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            tracing::warn!("bulk-sync: idle (no frame for 30s) before done; stopping");
            break;
        }
        let frame = match timeout(remaining.min(wait), read_frame(stream)).await {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => {
                tracing::warn!("bulk-sync read failed: {e}");
                break;
            }
            Err(_) => {
                info!("bulk-sync: no response (phone predates BULK_SYNC?); skipping");
                break;
            }
        };
        // Progress made → slide the idle deadline forward.
        deadline = tokio::time::Instant::now() + IDLE;
        let mut pt = vec![0u8; frame.payload.len()];
        let n = transport.read_message(&frame.payload, &mut pt)?;
        pt.truncate(n);
        match frame.ty {
            ty::BULK_SYNC if frame.sub == 0x02 => {
                info!("← bulk-sync done: {}", redact_done_frame(&pt));
                status = BulkStatus::parse(&pt);
                if status.is_none() {
                    tracing::warn!("bulk-sync: done frame is not a JSON object; no status");
                }
                offers = parse_offers(&pt);
                if !offers.is_empty() {
                    info!("← {} file offer(s) announced over LAN", offers.len());
                }
                deleted = parse_deleted(&pt);
                if !deleted.is_empty() {
                    info!("← {} capture(s) deleted on the phone", deleted.len());
                }
                clipboard = parse_clipboard(&pt);
                if let Some(c) = clipboard.as_ref() {
                    info!(chars = c.text.chars().count(), "← LAN clipboard sync");
                }
                break;
            }
            ty::PHONE_FILES => {
                if let Some((total, idx, data)) = crate::core::phone_files::parse_chunk(&pt) {
                    if let Some(json) = listing.add(total, idx, data) {
                        info!("← phone folder listing ({} bytes)", json.len());
                        out.push((ty::PHONE_FILES, json));
                    }
                } else {
                    tracing::warn!("bulk-sync: malformed listing chunk; dropping");
                }
            }
            ty::CONTACTS => {
                if let Some((total, idx, data)) = crate::core::contacts::parse_chunk(&pt) {
                    if let Some(json) = contacts.add(total, idx, data) {
                        info!("← bulk-sync contacts ({} bytes)", json.len());
                        out.push((ty::CONTACTS, json));
                    }
                } else {
                    tracing::warn!("bulk-sync: malformed contacts chunk; dropping");
                }
            }
            ty::CALL_LOG => {
                if let Some((total, idx, data)) = crate::core::call_log::parse_chunk(&pt) {
                    if let Some(json) = call_log.add(total, idx, data) {
                        info!("← bulk-sync call log ({} bytes)", json.len());
                        out.push((ty::CALL_LOG, json));
                    }
                } else {
                    tracing::warn!("bulk-sync: malformed call-log chunk; dropping");
                }
            }
            ty::SMS => {
                if let Some((total, idx, data)) = crate::core::sms::parse_chunk(&pt) {
                    if let Some(json) = sms.add(total, idx, data) {
                        info!("← bulk-sync sms ({} bytes)", json.len());
                        out.push((ty::SMS, json));
                    }
                } else {
                    tracing::warn!("bulk-sync: malformed sms chunk; dropping");
                }
            }
            // History batches (watermark datasets) — merge semantics on the
            // UI side, hence the distinct frame types.
            ty::SMS_THREAD => {
                if let Some((total, idx, data)) = crate::core::sms::parse_chunk(&pt) {
                    if let Some(json) = sms_history.add(total, idx, data) {
                        info!("← bulk-sync sms history ({} bytes)", json.len());
                        out.push((ty::SMS_THREAD, json));
                    }
                } else {
                    tracing::warn!("bulk-sync: malformed sms-history chunk; dropping");
                }
            }
            ty::CALL_LOG_HISTORY => {
                if let Some((total, idx, data)) = crate::core::call_log::parse_chunk(&pt) {
                    if let Some(json) = call_log_history.add(total, idx, data) {
                        info!("← bulk-sync call-log history ({} bytes)", json.len());
                        out.push((ty::CALL_LOG_HISTORY, json));
                    }
                } else {
                    tracing::warn!("bulk-sync: malformed call-log-history chunk; dropping");
                }
            }
            ty::SMS_IDS => {
                if let Some((total, idx, data)) = crate::core::sms::parse_chunk(&pt) {
                    if let Some(json) = sms_ids.add(total, idx, data) {
                        info!("← bulk-sync sms ids ({} bytes)", json.len());
                        out.push((ty::SMS_IDS, json));
                    }
                } else {
                    tracing::warn!("bulk-sync: malformed sms-ids chunk; dropping");
                }
            }
            ty::CLIPBOARD_IMAGE => {
                // Reliable LAN pull of a phone-shared image (vs the lossy BLE
                // chunk path). `[total][idx][data]` like the other datasets.
                if let Some((total, idx, data)) = crate::core::sms::parse_chunk(&pt) {
                    if let Some(png) = clipboard_image.add(total, idx, data) {
                        info!("← bulk-sync clipboard image ({} bytes)", png.len());
                        out.push((ty::CLIPBOARD_IMAGE, png));
                    }
                } else {
                    tracing::warn!("bulk-sync: malformed clipboard-image chunk; dropping");
                }
            }
            ty::CLIPBOARD_FILE => {
                // Instant-share-style file pull. The assembler resets on completion, so
                // several files served back-to-back reassemble independently.
                if let Some((total, idx, data)) = crate::core::sms::parse_chunk(&pt) {
                    file_chunks_seen = file_chunks_seen.saturating_add(1);
                    if file_chunks_seen == 1 {
                        file_start = Some(tokio::time::Instant::now());
                    }
                    // Live progress for the transfer panel (chunks → %).
                    crate::core::file_progress::report(file_chunks_seen, total as u32);
                    // Between two chunks is the only place a pull can be
                    // interrupted, so this is where a cancel takes effect.
                    // Ending the whole exchange is the point: the phone is
                    // mid-stream, and reading the rest only to drop it is the
                    // wait the user just asked to be let out of. The session
                    // is disposable — the next heartbeat opens another.
                    if crate::core::file_progress::take_cancel() {
                        info!("file pull cancelled by the user after {file_chunks_seen} chunk(s)");
                        break;
                    }
                    if let Some(blob) = clipboard_file.add(total, idx, data) {
                        let secs = file_start
                            .map(|s| s.elapsed().as_secs_f64())
                            .unwrap_or(0.0)
                            .max(0.001);
                        let mbps = blob.len() as f64 / 1024.0 / 1024.0 / secs;
                        info!(
                            "← bulk-sync file ({} bytes) in {:.2}s = {:.1} MB/s",
                            blob.len(),
                            secs,
                            mbps
                        );
                        out.push((ty::CLIPBOARD_FILE, blob));
                        file_chunks_seen = 0; // reset for the next file in the stream
                        file_start = None;
                    }
                } else {
                    tracing::warn!("bulk-sync: malformed clipboard-file chunk; dropping");
                }
            }
            other => {
                tracing::warn!("bulk-sync: unexpected frame 0x{other:02x}; ignoring");
            }
        }
    }
    Ok(BulkExchange { datasets: out, status, offers, deleted, clipboard })
}


#[cfg(test)]
mod tests {
    use super::{parse_clipboard, parse_deleted, parse_offers, redact_done_frame, BulkStatus};

    /// The done frame carries offers beside the per-dataset outcomes, and the
    /// status map must not choke on the array sitting next to its strings.
    #[test]
    fn offers_ride_the_done_frame_beside_the_status() {
        let body = br#"{"contacts":"match","offers":[
            {"token":"abc123","name":"shot.jpg","bytes":10,"mime":"image/jpeg","kind":"screenshot"}
        ]}"#;
        let offers = parse_offers(body);
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].token, "abc123");
        assert_eq!(offers[0].subdir(), Some("Phone/Screenshots"));
        // The status map still reads its own fields and ignores the array.
        let s = BulkStatus::parse(body).unwrap();
        assert_eq!(s.get("contacts"), Some("match"));
    }

    /// Clipboard text rides the done frame beside everything else, and the
    /// three readers must each ignore the others. This is the phone's only
    /// way to deliver a copy when the BLE link never connects.
    #[test]
    fn clipboard_rides_the_done_frame_beside_the_rest() {
        let body = br#"{"contacts":"match","deleted":["x"],
            "offers":[{"token":"t","name":"a.jpg","bytes":1,"mime":"image/jpeg","kind":"photo"}],
            "clipboard":{"text":"hello laptop","ts":1700000000000}}"#;
        let clip = parse_clipboard(body).expect("clipboard parsed");
        assert_eq!(clip.text, "hello laptop");
        // The neighbours still read their own fields.
        assert_eq!(BulkStatus::parse(body).unwrap().get("contacts"), Some("match"));
        assert_eq!(parse_offers(body).len(), 1);
        assert_eq!(parse_deleted(body), vec!["x".to_string()]);
    }

    /// A frame with no clipboard, or an empty one, yields nothing — an empty
    /// string must not clear the laptop's clipboard.
    #[test]
    fn absent_or_empty_clipboard_is_not_delivered() {
        assert!(parse_clipboard(br#"{"contacts":"match"}"#).is_none());
        assert!(parse_clipboard(br#"{"clipboard":{"text":"","ts":1}}"#).is_none());
        assert!(parse_clipboard(br#"not json"#).is_none());
    }

    /// The done frame is logged verbatim, so the clipboard body must be
    /// replaced by its length before it ever reaches the log.
    #[test]
    fn done_frame_log_never_carries_clipboard_text() {
        let body = br#"{"contacts":"match","clipboard":{"text":"hunter2 is secret","ts":1}}"#;
        let logged = redact_done_frame(body);
        assert!(!logged.contains("hunter2"), "clipboard text leaked into the log: {logged}");
        assert!(logged.contains("17 chars"), "length should survive: {logged}");
        // Everything else is still there to read.
        assert!(logged.contains("contacts"));
    }

    /// Deletions ride the same frame as the offers and the status map, and
    /// each reader must ignore the other two.
    #[test]
    fn deletions_ride_the_done_frame_too() {
        let body = br#"{"contacts":"match","deleted":["abc123","def456"],
            "offers":[{"token":"t","name":"a.jpg","bytes":1,"mime":"image/jpeg","kind":"photo"}]}"#;
        assert_eq!(parse_deleted(body), vec!["abc123".to_string(), "def456".to_string()]);
        assert_eq!(parse_offers(body).len(), 1);
        assert_eq!(BulkStatus::parse(body).unwrap().get("contacts"), Some("match"));
    }

    /// A build that reports no deletions, and an array with an entry that is
    /// not a token: neither may cost the caller the rest.
    #[test]
    fn missing_or_malformed_deletions_are_survivable() {
        assert!(parse_deleted(br#"{"contacts":"match"}"#).is_empty());
        assert!(parse_deleted(b"not json").is_empty());
        assert_eq!(parse_deleted(br#"{"deleted":[7,"keepme"]}"#), vec!["keepme".to_string()]);
    }

    /// A done frame from a build that doesn't announce offers, and one whose
    /// array holds something unreadable: neither may cost the caller anything
    /// it could otherwise have had.
    #[test]
    fn missing_or_malformed_offers_are_survivable() {
        assert!(parse_offers(br#"{"contacts":"match"}"#).is_empty());
        assert!(parse_offers(b"not json at all").is_empty());
        // One good entry beside one that cannot be read: keep the good one.
        let mixed = br#"{"offers":[7,{"token":"t","name":"a.jpg","bytes":1,"mime":"image/jpeg","kind":"photo"}]}"#;
        let offers = parse_offers(mixed);
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].subdir(), Some("Phone/Photos"));
    }

    #[test]
    fn nomatch_is_unservable_and_sent_is_not() {
        let s = BulkStatus::parse(br#"{"contacts":"match","clipboard_file":"nomatch"}"#).unwrap();
        assert!(s.unservable("clipboard_file"));
        assert_eq!(s.get("contacts"), Some("match"));

        let s = BulkStatus::parse(br#"{"clipboard_file":"sent"}"#).unwrap();
        assert!(!s.unservable("clipboard_file"));
    }

    #[test]
    fn a_dataset_the_phone_said_nothing_about_is_not_unservable() {
        // Never drop a queued pull on silence — only on an explicit answer.
        let s = BulkStatus::parse(br#"{"contacts":"match"}"#).unwrap();
        assert!(!s.unservable("clipboard_file"));
        assert_eq!(s.get("clipboard_file"), None);
    }

    #[test]
    fn errors_count_as_unservable_and_junk_parses_to_none() {
        let s = BulkStatus::parse(br#"{"clipboard_file":"error"}"#).unwrap();
        assert!(s.unservable("clipboard_file"));
        // Non-string values are skipped, not fatal.
        let s = BulkStatus::parse(br#"{"clipboard_file":"nomatch","n":7}"#).unwrap();
        assert!(s.unservable("clipboard_file"));
        assert!(BulkStatus::parse(b"[]").is_none());
        assert!(BulkStatus::parse(b"not json").is_none());
    }
}

/// The laptop → phone push, end to end over a loopback socket: the real
/// `push_outgoing_batch` against a fake phone that speaks the same Noise
/// transport and pulls with the same FS ops the Android `FsClient` sends.
#[cfg(test)]
mod push_tests {
    use super::*;
    use crate::core::ble::frame::sub;
    use crate::core::crypto::noise::NOISE_IK;
    use crate::core::crypto::x25519::public_from_private;
    use crate::core::fs_proto as p;
    use crate::core::outgoing_share::{decision, FolderZip, OutgoingFile, PUSH_CHUNK_BYTES};
    use crate::core::share_grants;
    use tokio::net::TcpListener;

    /// Two transports that completed IK with each other in memory, so the test
    /// can skip the handshake frames and start at the offer.
    fn transports() -> (snow::TransportState, snow::TransportState) {
        let laptop_priv = [7u8; 32];
        let phone_priv = [9u8; 32];
        let phone_pub = public_from_private(&phone_priv);
        let mut init = snow::Builder::new(NOISE_IK.parse().unwrap())
            .local_private_key(&laptop_priv)
            .unwrap()
            .remote_public_key(&phone_pub)
            .unwrap()
            .build_initiator()
            .unwrap();
        let mut resp = snow::Builder::new(NOISE_IK.parse().unwrap())
            .local_private_key(&phone_priv)
            .unwrap()
            .build_responder()
            .unwrap();
        let (mut a, mut b) = (vec![0u8; 1024], vec![0u8; 1024]);
        let n = init.write_message(&[], &mut a).unwrap();
        resp.read_message(&a[..n], &mut b).unwrap();
        let n = resp.write_message(&[], &mut a).unwrap();
        init.read_message(&a[..n], &mut b).unwrap();
        (init.into_transport_mode().unwrap(), resp.into_transport_mode().unwrap())
    }

    async fn pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (c, s) = tokio::join!(TcpStream::connect(addr), l.accept());
        (c.unwrap(), s.unwrap().0)
    }

    struct Phone {
        s: TcpStream,
        t: snow::TransportState,
        next_id: u32,
    }

    impl Phone {
        async fn recv(&mut self) -> (u8, u8, Vec<u8>) {
            let f = read_frame(&mut self.s).await.unwrap();
            let mut pt = vec![0u8; f.payload.len()];
            let n = self.t.read_message(&f.payload, &mut pt).unwrap();
            pt.truncate(n);
            (f.ty, f.sub, pt)
        }
        async fn send(&mut self, ty_byte: u8, sub: u8, plain: &[u8]) {
            let mut ct = vec![0u8; plain.len() + 16];
            let n = self.t.write_message(plain, &mut ct).unwrap();
            ct.truncate(n);
            write_frame(&mut self.s, &Frame::new(ty_byte, sub, ct)).await.unwrap();
        }
        async fn fs(&mut self, op: u8, mut req: serde_json::Value) -> (u8, Vec<u8>) {
            self.next_id += 1;
            req["id"] = self.next_id.into();
            self.send(ty::FS_REQ, op, req.to_string().as_bytes()).await;
            let (ty_byte, _, pt) = self.recv().await;
            (ty_byte, pt)
        }
        /// Pull one `share:` path the way `FsClient.readAll` does.
        async fn pull(&mut self, path: &str) -> Result<Vec<u8>, i32> {
            let (ty_byte, pt) = self.fs(p::op::OPEN, serde_json::json!({"path": path})).await;
            if ty_byte == ty::FS_ERR {
                return Err(serde_json::from_slice::<p::FsErr>(&pt).unwrap().code);
            }
            let handle = match serde_json::from_slice::<p::FsReply>(&pt).unwrap() {
                p::FsReply::Open { handle, .. } => handle,
                other => panic!("expected an open reply, got {other:?}"),
            };
            let mut out = Vec::new();
            loop {
                let (ty_byte, pt) = self
                    .fs(
                        p::op::READ,
                        serde_json::json!({"handle": handle, "offset": out.len(), "len": p::MAX_READ_LEN}),
                    )
                    .await;
                assert_eq!(ty_byte, ty::FS_DATA);
                let (_, off, eof, bytes) = p::decode_data(&pt).unwrap();
                assert_eq!(off as usize, out.len());
                out.extend_from_slice(bytes);
                if eof {
                    break;
                }
            }
            self.fs(p::op::CLOSE, serde_json::json!({"handle": handle})).await;
            Ok(out)
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vortex-push-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Bigger than any old cap would care about is not needed to prove the
    /// point: what matters is several ranges and a short tail.
    fn content(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn file(path: &std::path::Path, folder: Option<FolderZip>) -> OutgoingFile {
        OutgoingFile {
            name: path.file_name().unwrap().to_string_lossy().to_string(),
            mime: "application/octet-stream".into(),
            path: path.to_path_buf(),
            size: std::fs::metadata(path).unwrap().len(),
            folder,
        }
    }

    #[tokio::test]
    async fn the_phone_pulls_every_file_byte_exact_and_grants_end_with_the_batch() {
        let dir = scratch("pull");
        let a = dir.join("Formation au CAP électricien.pdf");
        let b = dir.join("empty.txt");
        let z = dir.join("123-ours.zip");
        std::fs::write(&a, content(3 * p::MAX_READ_LEN as usize + 17)).unwrap();
        std::fs::write(&b, b"").unwrap();
        std::fs::write(&z, content(1000)).unwrap();
        let files = vec![
            file(&a, None),
            file(&b, None),
            file(&z, Some(FolderZip { unpacked: 5000, entries: 3 })),
        ];

        let (mut laptop, phone_sock) = pair().await;
        let (mut lt, pt) = transports();
        let mut phone = Phone { s: phone_sock, t: pt, next_id: 0 };

        let phone_task = tokio::spawn(async move {
            let (ty_byte, _, offer) = phone.recv().await;
            assert_eq!(ty_byte, ty::FILE_PUSH_OFFER);
            let offer: serde_json::Value = serde_json::from_slice(&offer).unwrap();
            assert_eq!(offer["pull"], true);
            phone.send(ty::FILE_PUSH_DECISION, 0, &[decision::ACCEPT_PULL]).await;
            let mut got = Vec::new();
            let mut paths = Vec::new();
            for f in offer["files"].as_array().unwrap() {
                let path = f["path"].as_str().unwrap().to_string();
                got.push(phone.pull(&path).await.unwrap());
                paths.push(path);
            }
            // A guessed token opens nothing.
            assert_eq!(phone.pull("share:00000000000000000000000000000000").await, Err(p::code::NOENT));
            let report = serde_json::json!({"saved": got.len(), "failed": []});
            phone.send(ty::FILE_PUSH, sub::FILE_PUSH_DONE, report.to_string().as_bytes()).await;
            (offer, got, paths)
        });

        push_outgoing_batch(&mut laptop, &mut lt, &files).await.unwrap();
        let (offer, got, paths) = phone_task.await.unwrap();

        assert_eq!(got[0], std::fs::read(&a).unwrap(), "byte-exact across ranges");
        assert!(got[1].is_empty(), "an empty file is a file, not a skip");
        assert_eq!(got[2], content(1000));
        // The folder archive carries the totals the phone unpacks against.
        assert_eq!(offer["files"][2]["extract"], true);
        assert_eq!(offer["files"][2]["unpacked"], 5000);
        assert_eq!(offer["files"][2]["entries"], 3);
        assert_eq!(offer["files"][0]["extract"], false);
        assert!(offer["files"][0].get("unpacked").is_none());
        // The batch is over: no grant still opens, our archive is gone, the
        // user's files are untouched.
        for path in &paths {
            assert!(share_grants::get(share_grants::token_of(path).unwrap()).is_none());
        }
        assert!(!z.exists(), "temp archive discarded");
        assert!(a.exists() && b.exists(), "user files never deleted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_old_phone_gets_the_legacy_chunk_stream_read_from_disk() {
        let dir = scratch("legacy");
        let a = dir.join("a.bin");
        std::fs::write(&a, content(2 * PUSH_CHUNK_BYTES + 5)).unwrap();
        let files = vec![file(&a, None)];

        let (mut laptop, phone_sock) = pair().await;
        let (mut lt, pt) = transports();
        let mut phone = Phone { s: phone_sock, t: pt, next_id: 0 };

        let phone_task = tokio::spawn(async move {
            let (ty_byte, _, _) = phone.recv().await;
            assert_eq!(ty_byte, ty::FILE_PUSH_OFFER);
            phone.send(ty::FILE_PUSH_DECISION, 0, &[decision::ACCEPT_STREAM]).await;
            let mut out = Vec::new();
            loop {
                let (ty_byte, _, c) = phone.recv().await;
                assert_eq!(ty_byte, ty::FILE_PUSH);
                let total = u16::from_be_bytes([c[0], c[1]]);
                let idx = u16::from_be_bytes([c[2], c[3]]);
                out.extend_from_slice(&c[4..]);
                if idx + 1 == total {
                    break;
                }
            }
            out
        });

        push_outgoing_batch(&mut laptop, &mut lt, &files).await.unwrap();
        assert_eq!(phone_task.await.unwrap(), std::fs::read(&a).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_decline_sends_nothing_and_still_cleans_up() {
        let dir = scratch("decline");
        let z = dir.join("999-ours.zip");
        std::fs::write(&z, b"zip").unwrap();
        let files = vec![file(&z, Some(FolderZip { unpacked: 3, entries: 1 }))];

        let (mut laptop, phone_sock) = pair().await;
        let (mut lt, pt) = transports();
        let mut phone = Phone { s: phone_sock, t: pt, next_id: 0 };

        let phone_task = tokio::spawn(async move {
            let (_, _, offer) = phone.recv().await;
            let offer: serde_json::Value = serde_json::from_slice(&offer).unwrap();
            phone.send(ty::FILE_PUSH_DECISION, 0, &[decision::DECLINE]).await;
            offer["files"][0]["path"].as_str().unwrap().to_string()
        });

        push_outgoing_batch(&mut laptop, &mut lt, &files).await.unwrap();
        let path = phone_task.await.unwrap();
        assert!(share_grants::get(share_grants::token_of(&path).unwrap()).is_none());
        assert!(!z.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
