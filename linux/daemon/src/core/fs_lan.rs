//! A LAN transport for the filesystem protocol.
//!
//! BLE carries these frames today and works, but at 30-40 KiB/s (measured) —
//! fine for a directory listing, hopeless for content. Design doc §6 is
//! explicit that content streams over Wi-Fi and BLE stays for metadata and
//! wake-up, so this opens a TCP+IK session and pushes the same frames down it.
//!
//! Two properties make it worth a dedicated session rather than reusing the
//! heartbeat:
//!
//! * **It stays open.** The heartbeat connects, syncs and disconnects every
//!   ~13 s. Filesystem work is bursty and correlated — a fetch is one OPEN, N
//!   READs and a CLOSE — and paying a TCP connect plus an IK handshake
//!   (~300 ms on this link) per request would cost more than the reads.
//! * **No fragmentation.** A BLE notify caps at 512 bytes, so a 48 KiB read is
//!   96 fragments paced 10 ms apart. Over TCP the same read is one frame.
//!
//! The session is otherwise deliberately dumb: it moves frames and knows
//! nothing about ops, ids or handles. The caller keeps all of that, which is
//! what lets the same `fs_link` client sit on either transport.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::core::ble::frame::{ty, Frame, FRAME_HEADER_LEN, MAX_FRAME_PAYLOAD};
use crate::core::crypto::x25519::X25519SecBytes;

/// Budget for the handshake. Same shape as the audio session's: generous
/// enough for a sleepy phone, short enough that a dead address fails fast and
/// the caller can fall back to BLE while the user is still watching.
const IK_STEP_TIMEOUT: Duration = Duration::from_secs(8);

/// Writes one sealed frame onto the session.
pub type FsLanWriter = Arc<
    dyn Fn(u8, u8, Vec<u8>) -> futures::future::BoxFuture<'static, Result<(), String>>
        + Send
        + Sync,
>;

/// Open a TCP+IK session for filesystem frames.
///
/// On success the read loop runs until the socket closes or a frame fails to
/// open, handing every decrypted frame to `on_frame` and calling `on_closed`
/// exactly once at the end — the caller uses that to drop its cached writer so
/// the next request reopens or falls back rather than writing into a dead
/// socket.
#[allow(clippy::too_many_arguments)]
pub async fn open_session(
    addr: SocketAddr,
    static_priv: &X25519SecBytes,
    peer_static_pub: &[u8; 32],
    prs: &[u8; 32],
    local_counter: u64,
    on_frame: Arc<dyn Fn(Frame) + Send + Sync>,
    on_closed: Arc<dyn Fn() + Send + Sync>,
) -> Result<FsLanWriter, String> {
    let mut stream = timeout(IK_STEP_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| "tcp connect timeout".to_string())?
        .map_err(|e| format!("tcp connect: {e}"))?;
    // Filesystem traffic is many small round trips (a stat per icon) plus a
    // few large ones. Nagle would sit on the small ones waiting for company.
    let _ = stream.set_nodelay(true);

    let mut handshake =
        crate::core::lan::tcp_client::build_ik_initiator(static_priv, peer_static_pub, prs)
            .map_err(|e| format!("noise build: {e}"))?;
    let mut buf = vec![0u8; 1024];
    let mut tmp = vec![0u8; 1024];

    let n = handshake
        .write_message(&local_counter.to_be_bytes(), &mut buf)
        .map_err(|e| format!("noise write msg1: {e}"))?;
    write_frame(&mut stream, &Frame::new(ty::RECONNECT_HANDSHAKE, 0x01, buf[..n].to_vec())).await?;

    let msg2 = timeout(IK_STEP_TIMEOUT, read_frame_capped(&mut stream, 128))
        .await
        .map_err(|_| "msg2 timeout".to_string())??;
    if msg2.ty != ty::RECONNECT_HANDSHAKE || msg2.sub != 0x02 {
        return Err(format!("unexpected msg2 ty=0x{:02x}", msg2.ty));
    }
    handshake
        .read_message(&msg2.payload, &mut tmp)
        .map_err(|e| format!("noise read msg2: {e}"))?;

    // The peer's static must be the one we trusted at pair time. Checked
    // before a single filesystem frame goes out: this session can be asked to
    // read the user's files, so "who is on the other end" is not a question to
    // answer optimistically.
    if handshake
        .get_remote_static()
        .ok_or_else(|| "no remote static after IK".to_string())?
        != peer_static_pub
    {
        return Err("peer static mismatch".to_string());
    }

    let transport = Arc::new(Mutex::new(
        handshake
            .into_transport_mode()
            .map_err(|e| format!("transport mode: {e}"))?,
    ));

    let (mut reader, writer_half) = stream.into_split();
    let writer_half = Arc::new(Mutex::new(writer_half));

    // Read loop. Owns the receive side of the cipher, so it never contends
    // with the writer for it beyond the shared mutex.
    {
        let transport = transport.clone();
        tokio::spawn(async move {
            loop {
                match read_sealed(&mut reader, &transport).await {
                    Ok(Some(frame)) => on_frame(frame),
                    Ok(None) => {
                        tracing::info!("fs-lan: peer closed the session");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("fs-lan: read loop ended: {e}");
                        break;
                    }
                }
            }
            on_closed();
        });
    }

    let writer: FsLanWriter = Arc::new(move |ty_byte: u8, sub: u8, plain: Vec<u8>| {
        let transport = transport.clone();
        let writer_half = writer_half.clone();
        // Noise nonces are implicit: the phone opens the k-th frame it receives
        // with nonce k, and a failed open does not advance it. So every nonce
        // sealed here must reach the wire, in sealing order, exactly once — a
        // frame that is sealed and then not sent leaves the phone one nonce
        // behind for the rest of the session: every later request is "AEAD
        // decrypt failed" there and a 20 s timeout here, which is what froze
        // Dolphin on DCIM/Camera.
        //
        // Two things used to break that. The seal happened under the transport
        // lock and the write under a separate one, with an `await` between
        // them; and callers DO drop this future part-way — `Vfs::read_range`
        // abandons its other in-flight reads on the first error, EOF or short
        // read — so a read parked on the write lock behind a 48 KiB write lost
        // its already-sealed frame. Hence:
        //
        // * the write half is locked BEFORE sealing and held until the frame is
        //   out, so sealing order is wire order (as the BLE writers in
        //   `audio_signal` do);
        // * the whole thing runs as its own task, so a caller that stops
        //   waiting cannot cut it off between seal and write, or half-way
        //   through `write_all` (which would corrupt the framing instead).
        //
        // The transport lock itself stays brief, so the read loop is not held
        // up behind a large write.
        let job = tokio::spawn(async move {
            if plain.len() + 16 > MAX_FRAME_PAYLOAD {
                return Err(format!("frame too large: {}", plain.len()));
            }
            let mut out = vec![0u8; plain.len() + 16];
            let mut w = writer_half.lock().await;
            let n = {
                let mut t = transport.lock().await;
                t.write_message(&plain, &mut out)
                    .map_err(|e| format!("aead seal: {e}"))?
            };
            let bytes = Frame::new(ty_byte, sub, out[..n].to_vec()).encode();
            w.write_all(&bytes).await.map_err(|e| format!("tcp write: {e}"))?;
            w.flush().await.map_err(|e| format!("tcp flush: {e}"))?;
            Ok(())
        });
        Box::pin(async move { job.await.map_err(|e| format!("fs-lan write task: {e}"))? })
    });

    tracing::info!(%addr, "fs-lan: session up");
    Ok(writer)
}

async fn write_frame(stream: &mut TcpStream, frame: &Frame) -> Result<(), String> {
    let bytes = frame.encode();
    stream.write_all(&bytes).await.map_err(|e| format!("tcp write: {e}"))?;
    stream.flush().await.map_err(|e| format!("tcp flush: {e}"))?;
    Ok(())
}

async fn read_frame_capped(stream: &mut TcpStream, cap: usize) -> Result<Frame, String> {
    let cap = cap.min(MAX_FRAME_PAYLOAD);
    let mut header = [0u8; FRAME_HEADER_LEN];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|e| format!("tcp read header: {e}"))?;
    let length = u16::from_be_bytes([header[2], header[3]]) as usize;
    if length > cap {
        return Err(format!("oversize frame {length}"));
    }
    let mut full = vec![0u8; FRAME_HEADER_LEN + length];
    full[..FRAME_HEADER_LEN].copy_from_slice(&header);
    if length > 0 {
        stream
            .read_exact(&mut full[FRAME_HEADER_LEN..])
            .await
            .map_err(|e| format!("tcp read body: {e}"))?;
    }
    Frame::decode(&full).map_err(|e| format!("frame decode: {e}"))
}

/// Read one frame and AEAD-open it. `Ok(None)` is a clean EOF.
async fn read_sealed(
    reader: &mut tokio::net::tcp::OwnedReadHalf,
    transport: &Arc<Mutex<snow::TransportState>>,
) -> Result<Option<Frame>, String> {
    let mut header = [0u8; FRAME_HEADER_LEN];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(format!("tcp read header: {e}")),
    }
    let length = u16::from_be_bytes([header[2], header[3]]) as usize;
    if length > MAX_FRAME_PAYLOAD {
        return Err(format!("oversize frame {length}"));
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader
            .read_exact(&mut body)
            .await
            .map_err(|e| format!("tcp read body: {e}"))?;
    }
    let mut plain = vec![0u8; length.max(16)];
    let n = {
        let mut t = transport.lock().await;
        t.read_message(&body, &mut plain)
            .map_err(|e| format!("aead open: {e}"))?
    };
    Ok(Some(Frame::new(header[0], header[1], plain[..n].to_vec())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::crypto::noise::{prologue_with_prs, NOISE_IK};
    use crate::core::crypto::x25519::public_from_private;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    /// A listener whose accepted sockets keep a small, fixed receive buffer.
    ///
    /// Left to autotune, Linux grows a loopback receive buffer up to
    /// `tcp_rmem`'s maximum — 32 MiB on CachyOS — which can swallow everything
    /// the filler below sends, so nothing jams and the test proves nothing.
    /// Setting the size explicitly turns autotuning off, and accepted sockets
    /// inherit it from the listener.
    fn small_buffer_listener() -> TcpListener {
        let s = tokio::net::TcpSocket::new_v4().unwrap();
        s.set_recv_buffer_size(64 * 1024).unwrap();
        s.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        s.listen(1).unwrap()
    }

    /// What the fake phone saw: the first byte of every frame it opened, and
    /// whether it then hit one it could not.
    struct Seen {
        markers: Vec<u8>,
        failed: bool,
    }

    /// The phone's side, reduced to what matters here: answer IK, wait for
    /// `go`, then open every frame in the order it arrives — which is all a
    /// Noise transport can do, since its nonces are implicit.
    async fn fake_phone(
        listener: TcpListener,
        phone_priv: [u8; 32],
        prs: [u8; 32],
        go: oneshot::Receiver<()>,
    ) -> Seen {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut hs = snow::Builder::new(NOISE_IK.parse().unwrap())
            .local_private_key(&phone_priv)
            .unwrap()
            .prologue(&prologue_with_prs(&prs))
            .unwrap()
            .build_responder()
            .unwrap();
        let mut buf = vec![0u8; 1024];
        let msg1 = read_frame_capped(&mut s, 1024).await.unwrap();
        hs.read_message(&msg1.payload, &mut buf).unwrap();
        let n = hs.write_message(&[], &mut buf).unwrap();
        write_frame(&mut s, &Frame::new(ty::RECONNECT_HANDSHAKE, 0x02, buf[..n].to_vec()))
            .await
            .unwrap();
        let mut t = hs.into_transport_mode().unwrap();
        let _ = go.await;
        let mut seen = Seen {
            markers: Vec::new(),
            failed: false,
        };
        let mut plain = vec![0u8; MAX_FRAME_PAYLOAD];
        while let Ok(f) = read_frame_capped(&mut s, MAX_FRAME_PAYLOAD).await {
            match t.read_message(&f.payload, &mut plain) {
                Ok(n) if n > 0 => seen.markers.push(plain[0]),
                Ok(_) => {}
                Err(_) => {
                    seen.failed = true;
                    break;
                }
            }
        }
        seen
    }

    /// A send abandoned while queued must not cost the session its nonce sync.
    ///
    /// Regression: the writer sealed under the transport lock, then awaited
    /// the write lock. `Vfs::read_range` drops its other in-flight reads on the
    /// first error, EOF or short read, so a read parked behind a large write
    /// was dropped with its nonce already spent and its frame never sent. The
    /// phone, which opens frame k with nonce k and does not advance on a
    /// failure, then refused every later frame for the rest of the session —
    /// a file manager browsing DCIM/Camera froze on back-to-back 20 s timeouts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_abandoned_send_does_not_desync_the_phone() {
        const FILL: u8 = 1;
        const ABANDONED: u8 = 2;
        const AFTER: u8 = 3;
        let laptop_priv = [7u8; 32];
        let phone_priv = [9u8; 32];
        let prs = [3u8; 32];
        let listener = small_buffer_listener();
        let addr = listener.local_addr().unwrap();
        let (go_tx, go_rx) = oneshot::channel();
        let phone = tokio::spawn(fake_phone(listener, phone_priv, prs, go_rx));

        let w = open_session(
            addr,
            &laptop_priv,
            &public_from_private(&phone_priv),
            &prs,
            0,
            Arc::new(|_| {}),
            Arc::new(|| {}),
        )
        .await
        .unwrap();

        // Jam the write lock: more than the socket buffers hold, with the phone
        // not reading yet, so one large write sits in `write_all` holding it.
        let written = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let filler = {
            let w = w.clone();
            let written = written.clone();
            tokio::spawn(async move {
                // Errors ignored: a phone that lost sync hangs up, and the
                // assertion below is the one that should say so.
                for _ in 0..512 {
                    if w(ty::FS_REQ, 0, vec![FILL; 60 * 1024]).await.is_ok() {
                        written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            })
        };
        // Wait for the jam to be real rather than assume it after a fixed
        // delay: the kernel keeps growing the send buffer for a while, which
        // briefly unblocks the filler — and a send abandoned then would simply
        // get through. Progress that has stopped for half a second has hit the
        // buffers' ceiling.
        let mut last = usize::MAX;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let now = written.load(std::sync::atomic::Ordering::Relaxed);
            if now == last {
                break;
            }
            last = now;
        }

        // A request that gives up while queued behind it.
        let gave_up = tokio::time::timeout(
            Duration::from_millis(100),
            w(ty::FS_REQ, 0, vec![ABANDONED; 16]),
        )
        .await;
        assert!(gave_up.is_err(), "the write lock was not jammed; the test proves nothing");

        let _ = go_tx.send(());
        filler.await.unwrap();
        let _ = w(ty::FS_REQ, 0, vec![AFTER; 16]).await;
        drop(w);

        let seen = tokio::time::timeout(Duration::from_secs(30), phone)
            .await
            .expect("fake phone hung")
            .unwrap();
        assert!(
            !seen.failed,
            "the phone lost nonce sync after {} frames",
            seen.markers.len()
        );
        assert_eq!(seen.markers.last(), Some(&AFTER), "the last frame never arrived");
        assert_eq!(seen.markers.iter().filter(|&&m| m == FILL).count(), 512);
    }
}
