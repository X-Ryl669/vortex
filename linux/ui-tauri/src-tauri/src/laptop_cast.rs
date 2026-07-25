//! Screen-mirror SENDER (laptop → phone, view-only): capture the laptop screen
//! and stream it to the phone, which shows it in a viewer. The mirror image of
//! [`crate::mirror`] (which RECEIVES the phone's screen).
//!
//! Pipeline: the xdg-desktop-portal **ScreenCast** portal (Wayland-native, pops
//! the "share your screen" consent on the laptop) hands us a PipeWire node + fd;
//! GStreamer captures it (`pipewiresrc`), scales to 720p and encodes HEVC with
//! NVENC; an `appsink` hands each H.265 access unit to the daemon's
//! [`mirror_tcp::MirrorTcpSealer`], which seals it (ChaCha20-Poly1305, same wire
//! as the phone→laptop path) and serves it on [`mirror_tcp::LAPTOP_VIDEO_PORT`].
//! The phone connects out to it and decodes with MediaCodec.
//!
//! Crypto: the media key is derived from the live session's IK handshake hash
//! via [`mirror_udp::derive_laptop_media_key`] — a DISTINCT key from the
//! phone→laptop direction so the two streams never reuse a nonce. Both peers
//! derive it identically; nothing key-related goes on the wire.
//!
//! Trigger: the PHONE starts this (its "view laptop screen" button) — the portal
//! consent still appears on the laptop, but the user initiates from the phone.
//! One cast at a time; [`start`] replaces any prior session.

use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use ashpd::desktop::PersistMode;
use ashpd::WindowIdentifier;
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use rand::RngCore;
use tokio::sync::mpsc;

use vortex_l3_daemon::core::appstate::LaptopCast;
use vortex_l3_daemon::core::mirror_tcp;

/// Live cast handle: dropping/taking the stop sender ends the session (the cast
/// task selects on it). `None` = no cast running.
static CAST: Mutex<Option<CastHandle>> = Mutex::new(None);

/// The current cast's offer (ip, port, hex key) for the laptop's outgoing
/// AppState, so the phone knows where to dial. `None` when not casting.
static CAST_OFFER: Mutex<Option<LaptopCast>> = Mutex::new(None);

/// Edge-tracker for the phone's `laptop_mirror_req` level: we act only on the
/// false→true (start) and true→false (stop) transitions, ignoring the repeats
/// that arrive on every heartbeat.
static REQ_WANTED: AtomicBool = AtomicBool::new(false);

/// Consecutive `req == false` heartbeats seen while a cast is wanted. The phone
/// advertises `laptop_mirror_req` over BOTH BLE and LAN, sent at slightly
/// different times — right after the user taps, a STALE pre-tap snapshot
/// (req=false) can still arrive over the other transport and would otherwise
/// stop→restart the cast (new key → AEAD mismatch → black). We only honour a
/// stop after several consecutive falses; a genuine "close viewer" yields a
/// sustained false, while the startup race's lone stale false is outvoted by
/// the real req=true arriving on the other transport.
static REQ_FALSE_MISSES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Falses needed before we actually stop (≈ a few heartbeats of confirmed off).
const REQ_FALSE_LIMIT: u32 = 3;

struct CastHandle {
    /// Fire to tear the cast down (pipeline → Null, portal session closed).
    stop_tx: tokio::sync::oneshot::Sender<()>,
}

/// True while a laptop→phone cast is live.
pub fn active() -> bool {
    CAST.lock().map(|g| g.is_some()).unwrap_or(false)
}

/// The active cast offer for the laptop's outgoing AppState (`laptop_cast`).
pub fn current_offer() -> Option<LaptopCast> {
    CAST_OFFER.lock().ok().and_then(|g| g.clone())
}

/// Drive the cast from the phone's `laptop_mirror_req` level (called on every
/// inbound phone AppState, over LAN + BLE-STATE). Starts a fresh cast on the
/// false→true edge (random key, our LAN IP, portal consent) and stops on
/// true→false. Idempotent across the repeated heartbeats in between.
pub fn dispatch_request(req: bool) {
    // Any real request resets the stop-debounce: a single stale `false` between
    // genuine `true`s must not count toward a stop.
    if req {
        REQ_FALSE_MISSES.store(0, Ordering::SeqCst);
    } else if REQ_WANTED.load(Ordering::SeqCst) {
        // Casting but saw a `false`: only stop once it's CONFIRMED (sustained),
        // not on a lone stale snapshot from the other transport.
        if REQ_FALSE_MISSES.fetch_add(1, Ordering::SeqCst) + 1 < REQ_FALSE_LIMIT {
            return;
        }
    }
    if req && !REQ_WANTED.swap(true, Ordering::SeqCst) {
        // Rising edge. We DIAL the phone (it's the video server — only
        // laptop→phone connections survive real networks), so we need the
        // phone's IP from the live LAN session. Without it we can't reach the
        // viewer; bail and let a later heartbeat (with an IP) retry.
        let Some(phone_ip) = (match crate::lan::LAST_GOOD_PEER_IP.lock() {
            Ok(g) => *g,
            Err(_) => None,
        }) else {
            tracing::warn!("laptop-cast: no known phone IP yet — not starting");
            REQ_WANTED.store(false, Ordering::SeqCst);
            return;
        };
        // Fresh random media key (never derived/reused). The offer just carries
        // the key (+ port) so the phone opens the stream; the phone is the
        // server now, so no laptop IP is needed.
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        if let Ok(mut g) = CAST_OFFER.lock() {
            *g = Some(LaptopCast {
                ip: String::new(), // unused — the laptop dials the phone
                port: mirror_tcp::LAPTOP_VIDEO_PORT,
                key: hex::encode(key), // key material — logged nowhere
            });
        }
        tokio::spawn(async move {
            if let Err(e) = start(phone_ip, key).await {
                tracing::warn!("laptop-cast: start failed: {e}");
                if let Ok(mut g) = CAST_OFFER.lock() {
                    *g = None;
                }
                REQ_WANTED.store(false, Ordering::SeqCst);
            }
        });
    } else if !req && REQ_WANTED.swap(false, Ordering::SeqCst) {
        // Confirmed falling edge: the phone closed the viewer → release capture.
        REQ_FALSE_MISSES.store(0, Ordering::SeqCst);
        stop();
        if let Ok(mut g) = CAST_OFFER.lock() {
            *g = None;
        }
    }
}

/// Start casting the laptop screen to the phone with media key `key` (a fresh
/// random per-cast secret the caller also ships to the phone over the Noise-
/// sealed AppState). Pops the ScreenCast consent and serves the sealed HEVC
/// stream on [`mirror_tcp::LAPTOP_VIDEO_PORT`]. Replaces any prior cast. Returns
/// once the portal + pipeline are up (or an error if the user cancels consent /
/// capture can't start).
pub async fn start(phone_ip: std::net::IpAddr, key: [u8; 32]) -> Result<(), String> {
    stop();

    // ---- Portal: open a ScreenCast session and get the PipeWire node + fd. ----
    let proxy = Screencast::new()
        .await
        .map_err(|e| format!("portal connect: {e}"))?;
    let session = proxy
        .create_session()
        .await
        .map_err(|e| format!("portal session: {e}"))?;
    proxy
        .select_sources(
            &session,
            CursorMode::Embedded,
            SourceType::Monitor.into(),
            false,
            None,
            PersistMode::DoNot,
        )
        .await
        .map_err(|e| format!("portal select_sources: {e}"))?;
    let streams = proxy
        .start(&session, &WindowIdentifier::default())
        .await
        .map_err(|e| format!("portal start (consent declined?): {e}"))?
        .response()
        .map_err(|e| format!("portal start response: {e}"))?;
    let stream = streams
        .streams()
        .first()
        .ok_or_else(|| "portal returned no stream".to_string())?;
    let node_id = stream.pipe_wire_node_id();
    let fd = proxy
        .open_pipe_wire_remote(&session)
        .await
        .map_err(|e| format!("portal open_pipe_wire_remote: {e}"))?;
    tracing::info!(node_id, size = ?stream.size(), "laptop-cast: portal stream ready");

    // ---- GStreamer: capture → scale 720p → NVENC HEVC → appsink. ----
    if let Err(e) = gst::init() {
        return Err(format!("gst init: {e}"));
    }
    let raw_fd = fd.as_raw_fd();
    // CPU H.264 (x264enc), NOT a GPU encoder. On this hybrid laptop the GNOME
    // compositor + the portal capture run on the INTEL iGPU; feeding those
    // Intel PipeWire frames into NVENC (the NVIDIA dGPU) forces a cross-GPU
    // DMA-BUF import that FAULTS the compositor (it crashed gnome-shell → logged
    // the user out). Forcing `video/x-raw,format=I420` after `videoconvert`
    // pins the frames to SYSTEM MEMORY (no DMA-BUF reaches the encoder), so the
    // encode never touches a GPU context the compositor owns. `videorate` caps
    // a high-refresh panel to 30 fps — plenty for a screen view, light on CPU.
    let desc = format!(
        "pipewiresrc fd={raw_fd} path={node_id} do-timestamp=true keepalive-time=1000 ! \
         videorate ! videoconvert ! videoscale ! \
         video/x-raw,format=I420,width=1280,height=720,framerate=30/1 ! \
         x264enc tune=zerolatency speed-preset=veryfast bitrate=4000 key-int-max=30 ! \
         h264parse config-interval=-1 ! \
         video/x-h264,stream-format=byte-stream,alignment=au ! \
         appsink name=vsink emit-signals=false max-buffers=3 drop=true sync=false"
    );
    let pipeline = gst::parse::launch(&desc)
        .map_err(|e| format!("build pipeline: {e}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "pipeline downcast".to_string())?;

    // appsink → bounded channel of sealed-ready access units. The callback runs
    // on a GStreamer streaming thread (sync), so it `try_send`s and drops on a
    // full queue rather than blocking the encoder — keeps the stream live.
    let (au_tx, au_rx) = mpsc::channel::<Vec<u8>>(8);
    let appsink = pipeline
        .by_name("vsink")
        .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
        .ok_or_else(|| "appsink missing".to_string())?;
    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                if let Some(buf) = sample.buffer() {
                    if let Ok(map) = buf.map_readable() {
                        // Drop if the network can't keep up — never block here.
                        let _ = au_tx.try_send(map.as_slice().to_vec());
                    }
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    // Push the sealed stream to the phone (we dial it — the phone is the server).
    tokio::spawn(mirror_tcp::run_tcp_video_client(phone_ip, key, au_rx));

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| format!("pipeline play: {e}"))?;
    tracing::info!("laptop-cast: capturing + serving on {}", mirror_tcp::LAPTOP_VIDEO_PORT);

    // Own proxy/session/fd/pipeline for the cast's lifetime in one task; drop
    // them all (closing the portal + capture) when stopped or the bus errors.
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    {
        let mut g = CAST.lock().map_err(|_| "cast lock".to_string())?;
        *g = Some(CastHandle { stop_tx });
    }
    let bus = pipeline.bus();
    tokio::spawn(async move {
        // Keep these alive until teardown: dropping `fd`/`session` closes the
        // PipeWire stream and the portal session; `proxy` backs the session.
        let _keep = (proxy, session, fd);
        let mut stop_rx = stop_rx;
        loop {
            // Drain any pending bus messages WITHOUT blocking the async runtime
            // (`pop()` is non-blocking; the old `timed_pop` blocked a tokio
            // worker thread for 250ms a spin). Stop on a fatal error / EOS.
            let mut fatal = false;
            if let Some(bus) = &bus {
                while let Some(msg) = bus.pop() {
                    match msg.view() {
                        gst::MessageView::Error(e) => {
                            tracing::warn!("laptop-cast: pipeline error: {}", e.error());
                            fatal = true;
                        }
                        gst::MessageView::Eos(_) => fatal = true,
                        _ => {}
                    }
                }
            }
            if fatal {
                break;
            }
            // Wait for the next poll tick OR the user stop, whichever first.
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
            }
        }
        let _ = pipeline.set_state(gst::State::Null);
        if let Ok(mut g) = CAST.lock() {
            *g = None;
        }
        // Clear the offer so a pipeline error/EOS (not just a user stop) makes
        // the phone see a sustained `laptop_cast = None` → it closes its viewer
        // → its request drops → our `dispatch_request(false)` falling edge then
        // resets REQ_WANTED cleanly. We deliberately do NOT reset REQ_WANTED here:
        // doing so while the phone still wants the cast would immediately re-arm
        // a NEW cast (new key) under the still-open old viewer → AEAD mismatch.
        if let Ok(mut g) = CAST_OFFER.lock() {
            *g = None;
        }
        tracing::info!("laptop-cast: stopped (capture + portal released)");
    });

    Ok(())
}

/// Stop the live cast (if any): the task tears down the pipeline and releases
/// the portal/capture. Idempotent.
pub fn stop() {
    let taken = CAST.lock().ok().and_then(|mut g| g.take());
    if let Some(h) = taken {
        let _ = h.stop_tx.send(());
    }
}
