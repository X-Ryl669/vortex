//! Screen-mirror RECEIVER (laptop): decode the phone's HEVC (H.265) video and show it
//! in a native GStreamer window.
//!
//! All networking + crypto live in the daemon crate
//! (`core::mirror_session` for the TCP control plane, `core::mirror_udp` for the
//! sealed UDP video). This module is just the GStreamer half: it takes a channel
//! of decrypted, reassembled HEVC access units and pumps them into an `appsrc`,
//! plus the orchestration that wires a session together. Ported from the
//! upstream `ecosystem` `mirror_runtime.rs` (the raw-TCP receiver there is
//! replaced by the daemon's sealed transport here).
//!
//! Decoder backends, best-first: NVIDIA `nvh265dec` (full GPU path), VA-API
//! `vah265dec`, else software `avdec_h265`. XWayland target — glimagesink /
//! xvimagesink create their own X11 window.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use vortex_l3_daemon::core::crypto::x25519::X25519SecBytes;
use vortex_l3_daemon::core::mirror_session::{start_mirror_session, MirrorHandle, MirrorStart};
use vortex_l3_daemon::core::{mirror_tcp, mirror_udp};

/// The live control-session handle, kept alive for the stream's lifetime (its
/// channel senders must not drop or the session loops shut down). M3's X11 input
/// loop pushes into `input_tx`; `stop_mirror` takes + closes it.
static MIRROR_HANDLE: std::sync::Mutex<Option<MirrorHandle>> = std::sync::Mutex::new(None);

/// The live GStreamer pipeline. Held so a UI "Stop sharing" (and `cleanup`) can
/// set it to Null — which closes glimagesink's window — WITHOUT relying on the
/// bus EOS cascade or the window's own X button (whose hit-target is misplaced
/// under fractional display scaling, so clicking it does nothing).
static MIRROR_PIPELINE: std::sync::Mutex<Option<gst::Pipeline>> = std::sync::Mutex::new(None);

/// The phone's display name, set when a mirror starts so the GL window can be
/// retitled from glimagesink's default "OpenGL renderer" to the phone's name.
static MIRROR_TITLE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Sender clone of the live session's input channel, published while a mirror
/// is up so the GStreamer navigation pad-probe (which runs on a GStreamer
/// thread, far from `spawn_mirror`) can push laptop→phone input packets into
/// the sealed control plane. Cleared on `stop_mirror`.
static INPUT_TX: std::sync::Mutex<Option<mpsc::Sender<Vec<u8>>>> = std::sync::Mutex::new(None);

/// True while the left mouse button is held down inside the mirror window — a
/// press starts a touch, moves extend it, release ends it. Reset at session
/// start. Single mirror at a time, so a process-wide flag is enough.
static POINTER_DOWN: AtomicBool = AtomicBool::new(false);

/// Wheel / two-finger scroll is replayed on the phone as a CONTINUOUS finger
/// drag (so it rides the same smooth gesture pump as a real drag). These hold
/// the synthetic finger between scroll notches; an idle-watcher thread lifts the
/// finger once the wheel goes quiet. `SCROLL_SERIAL` bumps on every notch so the
/// watcher can detect "no scroll for a while" without clock math.
static SCROLL_ACTIVE: AtomicBool = AtomicBool::new(false);
/// The synthetic finger's current position (normalized), accumulated across
/// notches on BOTH axes — vertical (button 4/5) and horizontal (button 6/7), the
/// latter driving home-screen page swipes.
static SCROLL_FINGER_X: AtomicI32 = AtomicI32::new(0);
static SCROLL_FINGER_Y: AtomicI32 = AtomicI32::new(0);
/// Ensures the scroll idle-watcher thread is spawned exactly once.
static SCROLL_WATCHER: AtomicBool = AtomicBool::new(false);

/// Ctrl held (tracked from key events) → wheel becomes pinch-zoom, not scroll.
static CTRL_HELD: AtomicBool = AtomicBool::new(false);
/// Two-finger pinch-zoom state (Ctrl+wheel). The fingers sit above/below a fixed
/// centre; `PINCH_SPREAD` (their half-distance) grows to zoom in, shrinks to zoom
/// out. The same idle-watcher emits the MOVEs and lifts both fingers.
static PINCH_ACTIVE: AtomicBool = AtomicBool::new(false);
static PINCH_CX: AtomicI32 = AtomicI32::new(0);
static PINCH_CY: AtomicI32 = AtomicI32::new(0);
static PINCH_SPREAD: AtomicI32 = AtomicI32::new(0);
/// Initial half-spread between the two pinch fingers, and the change per notch.
const PINCH_INIT: i32 = 5_000;
const PINCH_NOTCH: i32 = 1_600;
const PINCH_MIN: i32 = 800;
const PINCH_MAX: i32 = 30_000;

/// Normalized units the synthetic finger travels per unit of a real
/// `mouse-scroll` delta (backends that emit one).
const SCROLL_GAIN: f64 = 11_000.0;

/// Normalized units the finger travels per discrete vertical notch (button
/// 4/5). High-res touchpads fire MANY of these per swipe, so keep it small or
/// scrolling races; the scroll-drag accumulates them and the watcher emits
/// real-touch MOVEs at a steady rate. Positive = finger DOWN. Bumped ~1.6× from
/// the original 950 so each wheel notch covers more screen (snappier scrolling).
const NOTCH_STEP: i32 = 1_500;

/// Larger per-notch step for HORIZONTAL notches (button 6/7): a home-screen page
/// flip needs the finger to cross a big fraction of the screen within one
/// scroll-drag burst, so each horizontal notch travels further. Positive =
/// finger RIGHT.
const NOTCH_STEP_X: i32 = 6_000;

/// laptop→phone input packet types (5-byte `[type][x_hi][x_lo][y_hi][y_lo]`,
/// x/y normalized to 0..=65535 of the video frame). The phone un-normalizes to
/// its real resolution and injects via its AccessibilityService. Touch is
/// DOWN→MOVE…→UP (a tap is DOWN+UP at one point); nav buttons ignore x/y.
mod input_proto {
    pub const DOWN: u8 = 0;
    pub const MOVE: u8 = 1;
    pub const UP: u8 = 2;
    pub const BACK: u8 = 10;
    #[allow(dead_code)]
    pub const HOME: u8 = 11;
    #[allow(dead_code)]
    pub const RECENTS: u8 = 12;
}

/// Map a sink-space pixel coordinate (0..span) to the normalized 0..=65535
/// wire range, clamping the letterboxed margins glimagesink reports outside the
/// active video rectangle.
fn norm(v: f64, span: u32) -> u16 {
    let s = span.max(1) as f64;
    (v / s * 65535.0).round().clamp(0.0, 65535.0) as u16
}

/// Enqueue a 5-byte input packet onto the live session's sealed control plane.
/// `try_send` (drop-on-full) is deliberate: the channel is bounded and MOVE
/// floods are coalescible — losing an intermediate move never breaks a gesture
/// (DOWN/UP are rare and always make it through).
fn send_input(ty: u8, nx: u16, ny: u16) {
    // Prefer the real-touch uinput injector (scrcpy-style: low latency, true
    // multitouch, bypasses MIUI's INJECT_EVENTS block). Fall back to the
    // AccessibilityService control plane when adb/the injector isn't available.
    if crate::mirror_inject::active() {
        let cmd = match ty {
            input_proto::DOWN => format!("D 0 {nx} {ny}"),
            input_proto::MOVE => format!("M 0 {nx} {ny}"),
            input_proto::UP => "U 0".to_string(),
            input_proto::BACK => "K back".to_string(),
            input_proto::HOME => "K home".to_string(),
            input_proto::RECENTS => "K recents".to_string(),
            _ => return,
        };
        crate::mirror_inject::send(&cmd);
        return;
    }
    let pkt = vec![ty, (nx >> 8) as u8, nx as u8, (ny >> 8) as u8, ny as u8];
    if let Ok(g) = INPUT_TX.lock() {
        if let Some(tx) = g.as_ref() {
            let _ = tx.try_send(pkt);
        }
    }
}

/// Translate a single GstNavigation event structure into wire packets. Coords
/// are already in the sink's display caps space (disp_w×disp_h) — glimagesink
/// and glcolorscale handle the window→video mapping incl. aspect letterboxing.
fn handle_nav(s: &gst::StructureRef, disp_w: u32, disp_h: u32) {
    let event = s.get::<String>("event").unwrap_or_default();
    let px = || s.get::<f64>("pointer_x").unwrap_or(0.0);
    let py = || s.get::<f64>("pointer_y").unwrap_or(0.0);
    match event.as_str() {
        "mouse-button-press" => {
            // On X11/XWayland a wheel / two-finger scroll arrives as button
            // presses — NOT a `mouse-scroll` event: 4 (up) / 5 (down) vertical,
            // 6 (left) / 7 (right) horizontal. Route them into the scroll-drag;
            // horizontal drives home-screen page swipes.
            match s.get::<i32>("button").unwrap_or(0) {
                1 => {
                    flush_scroll(); // a click during inertial scroll ends it
                    flush_pinch();
                    POINTER_DOWN.store(true, Ordering::Relaxed);
                    send_input(input_proto::DOWN, norm(px(), disp_w), norm(py(), disp_h));
                }
                4 if CTRL_HELD.load(Ordering::Relaxed) => feed_pinch(norm(px(), disp_w), norm(py(), disp_h), 1),
                5 if CTRL_HELD.load(Ordering::Relaxed) => feed_pinch(norm(px(), disp_w), norm(py(), disp_h), -1),
                4 => feed_scroll(norm(px(), disp_w), norm(py(), disp_h), 0, NOTCH_STEP),
                5 => feed_scroll(norm(px(), disp_w), norm(py(), disp_h), 0, -NOTCH_STEP),
                6 => feed_scroll(norm(px(), disp_w), norm(py(), disp_h), -NOTCH_STEP_X, 0),
                7 => feed_scroll(norm(px(), disp_w), norm(py(), disp_h), NOTCH_STEP_X, 0),
                _ => {}
            }
        }
        "mouse-button-release" if s.get::<i32>("button").unwrap_or(0) == 1 => {
            if POINTER_DOWN.swap(false, Ordering::Relaxed) {
                send_input(input_proto::UP, norm(px(), disp_w), norm(py(), disp_h));
            }
        }
        "mouse-move" if POINTER_DOWN.load(Ordering::Relaxed) => {
            send_input(input_proto::MOVE, norm(px(), disp_w), norm(py(), disp_h));
        }
        "mouse-scroll" => {
            // Some backends DO emit a real scroll event — handle both axes.
            let dx = s.get::<f64>("delta_pointer_x").unwrap_or(0.0);
            let dy = s.get::<f64>("delta_pointer_y").unwrap_or(0.0);
            if dx != 0.0 || dy != 0.0 {
                feed_scroll(
                    norm(px(), disp_w),
                    norm(py(), disp_h),
                    (dx * SCROLL_GAIN) as i32,
                    (dy * SCROLL_GAIN) as i32,
                );
            }
        }
        "key-press" => {
            let key = s.get::<String>("key").unwrap_or_default();
            if key == "Control_L" || key == "Control_R" {
                CTRL_HELD.store(true, Ordering::Relaxed); // wheel → pinch-zoom
            } else {
                inject_key(&key);
            }
        }
        "key-release" => {
            let key = s.get::<String>("key").unwrap_or_default();
            if key == "Control_L" || key == "Control_R" {
                CTRL_HELD.store(false, Ordering::Relaxed);
            }
        }
        _ => {}
    }
}

/// Linux key codes for letters a..z (QWERTY positional, NOT alphabetical).
const LETTER_CODES: [u16; 26] = [
    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45,
    21, 44,
];
/// Linux key codes for digits 0..9.
const DIGIT_CODES: [u16; 10] = [11, 2, 3, 4, 5, 6, 7, 8, 9, 10];
const KEY_LEFTSHIFT: u16 = 42;

/// Map a GstNavigation key name (X11 keysym / character) to a Linux key code and
/// whether Shift is needed. Covers letters, digits, common punctuation and the
/// editing/navigation keys — enough for real typing into phone text fields.
fn key_to_linux(key: &str) -> Option<(u16, bool)> {
    if key.chars().count() == 1 {
        let ch = key.chars().next().unwrap();
        if ch.is_ascii_alphabetic() {
            return Some((
                LETTER_CODES[(ch.to_ascii_lowercase() as u8 - b'a') as usize],
                ch.is_ascii_uppercase(),
            ));
        }
        if ch.is_ascii_digit() {
            return Some((DIGIT_CODES[(ch as u8 - b'0') as usize], false));
        }
        return match ch {
            ' ' => Some((57, false)),
            '.' => Some((52, false)),
            ',' => Some((51, false)),
            '-' => Some((12, false)),
            '=' => Some((13, false)),
            '/' => Some((53, false)),
            ';' => Some((39, false)),
            '\'' => Some((40, false)),
            '@' => Some((3, true)),
            '!' => Some((2, true)),
            '?' => Some((53, true)),
            ':' => Some((39, true)),
            '_' => Some((12, true)),
            _ => None,
        };
    }
    match key {
        "space" => Some((57, false)),
        "Return" | "KP_Enter" => Some((28, false)),
        "BackSpace" => Some((14, false)),
        "Tab" => Some((15, false)),
        "Left" => Some((105, false)),
        "Right" => Some((106, false)),
        "Up" => Some((103, false)),
        "Down" => Some((108, false)),
        "period" => Some((52, false)),
        "comma" => Some((51, false)),
        "minus" => Some((12, false)),
        _ => None,
    }
}

/// Inject a keystroke. Esc maps to Android Back. Real keys ride the uinput
/// injector (a complete down/up, Shift-wrapped) — only available there; with the
/// accessibility fallback only Esc→Back works (no text injection).
fn inject_key(key: &str) {
    if key == "Escape" {
        send_input(input_proto::BACK, 0, 0);
        return;
    }
    if !crate::mirror_inject::active() {
        return;
    }
    if let Some((code, shift)) = key_to_linux(key) {
        if shift {
            crate::mirror_inject::send(&format!("E {KEY_LEFTSHIFT} 1"));
        }
        crate::mirror_inject::send(&format!("E {code} 1"));
        crate::mirror_inject::send(&format!("E {code} 0"));
        if shift {
            crate::mirror_inject::send(&format!("E {KEY_LEFTSHIFT} 0"));
        }
    }
}

/// Feed one scroll increment into the synthetic scroll-drag. The first
/// increment presses a finger at the cursor; each one slides it by
/// `(step_x, step_y)` (already-signed normalized units, +x = RIGHT, +y = DOWN);
/// the idle-watcher lifts it once scrolling stops. The phone replays this as one
/// continuous drag — vertical = scroll, horizontal = home-screen page swipe.
fn feed_scroll(nx: u16, ny: u16, step_x: i32, step_y: i32) {
    ensure_scroll_watcher();
    if !SCROLL_ACTIVE.swap(true, Ordering::Relaxed) {
        tracing::info!(nx, ny, "mirror: scroll-drag started");
        SCROLL_FINGER_X.store(nx as i32, Ordering::Relaxed);
        SCROLL_FINGER_Y.store(ny as i32, Ordering::Relaxed);
        send_input(input_proto::DOWN, nx, ny);
    }
    // Only ACCUMULATE the target here — a high-res touchpad fires notches far
    // faster than we should write to the adb pipe. The watcher emits real-touch
    // MOVEs from this position at a steady ~60Hz, so the pipe never floods (the
    // cause of the stutter) and the motion stays smooth.
    let new_x = (SCROLL_FINGER_X.load(Ordering::Relaxed) + step_x).clamp(0, 65_535);
    let new_y = (SCROLL_FINGER_Y.load(Ordering::Relaxed) + step_y).clamp(0, 65_535);
    SCROLL_FINGER_X.store(new_x, Ordering::Relaxed);
    SCROLL_FINGER_Y.store(new_y, Ordering::Relaxed);
}

/// Lift the synthetic scroll finger now (called when scrolling goes idle, a real
/// click begins, or the session stops). No-op if no scroll drag is active.
fn flush_scroll() {
    if SCROLL_ACTIVE.swap(false, Ordering::Relaxed) {
        let fx = SCROLL_FINGER_X.load(Ordering::Relaxed).clamp(0, 65_535) as u16;
        let fy = SCROLL_FINGER_Y.load(Ordering::Relaxed).clamp(0, 65_535) as u16;
        send_input(input_proto::UP, fx, fy);
    }
}

/// Feed one Ctrl+wheel notch into a two-finger pinch-zoom. `dir` +1 = zoom in
/// (fingers spread apart), -1 = zoom out. Real multitouch (uinput) only.
fn feed_pinch(cx: u16, cy: u16, dir: i32) {
    if !crate::mirror_inject::active() {
        return;
    }
    ensure_scroll_watcher();
    if !PINCH_ACTIVE.swap(true, Ordering::Relaxed) {
        flush_scroll(); // never pinch + scroll at once
        PINCH_CX.store(cx as i32, Ordering::Relaxed);
        PINCH_CY.store(cy as i32, Ordering::Relaxed);
        PINCH_SPREAD.store(PINCH_INIT, Ordering::Relaxed);
        let top = (cy as i32 - PINCH_INIT).clamp(0, 65_535);
        let bot = (cy as i32 + PINCH_INIT).clamp(0, 65_535);
        crate::mirror_inject::send(&format!("D 0 {cx} {top}"));
        crate::mirror_inject::send(&format!("D 1 {cx} {bot}"));
    }
    let s = (PINCH_SPREAD.load(Ordering::Relaxed) + dir * PINCH_NOTCH).clamp(PINCH_MIN, PINCH_MAX);
    PINCH_SPREAD.store(s, Ordering::Relaxed);
}

/// Lift both pinch fingers (idle, click, or session stop). No-op if inactive.
fn flush_pinch() {
    if PINCH_ACTIVE.swap(false, Ordering::Relaxed) {
        crate::mirror_inject::send("U 0");
        crate::mirror_inject::send("U 1");
    }
}

/// Spawn (once) the watcher that ends a scroll drag after the wheel goes quiet
/// (~120ms with no new notch), giving the phone a clean finger-up so its fling
/// settles. Sampling `SCROLL_SERIAL` avoids any clock arithmetic.
fn ensure_scroll_watcher() {
    if SCROLL_WATCHER.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(|| {
        // ~80ms of quiet before lifting: short enough that the finger lifts while
        // the recent motion's velocity is still fresh, so Android computes a real
        // FLING (inertial scroll) — and the app renders the fling itself instead
        // of us dragging it frame-by-frame, which is smoother AND lighter.
        // ~190ms: long enough that a continuous touchpad/wheel scroll stays ONE
        // drag (no lift/re-press breaks between notch bursts — those read as
        // "uzilish"/stutter), short enough to stay responsive at the end.
        const IDLE_TICKS: u32 = 12;
        let mut last_x = -1i32;
        let mut last_y = -1i32;
        let mut last_spread = i32::MIN;
        let mut idle = 0u32;
        loop {
            // ~60Hz: steady emission rate decoupled from the touchpad's notch
            // rate, so the adb pipe never backs up.
            std::thread::sleep(Duration::from_millis(16));
            if SCROLL_ACTIVE.load(Ordering::Relaxed) {
                let tx = SCROLL_FINGER_X.load(Ordering::Relaxed);
                let ty = SCROLL_FINGER_Y.load(Ordering::Relaxed);
                if tx != last_x || ty != last_y {
                    send_input(input_proto::MOVE, tx.clamp(0, 65_535) as u16, ty.clamp(0, 65_535) as u16);
                    last_x = tx;
                    last_y = ty;
                    idle = 0;
                } else {
                    idle += 1;
                    if idle >= IDLE_TICKS {
                        flush_scroll();
                        idle = 0;
                    }
                }
            } else if PINCH_ACTIVE.load(Ordering::Relaxed) {
                let sp = PINCH_SPREAD.load(Ordering::Relaxed);
                if sp != last_spread {
                    let cx = PINCH_CX.load(Ordering::Relaxed);
                    let cy = PINCH_CY.load(Ordering::Relaxed);
                    let top = (cy - sp).clamp(0, 65_535);
                    let bot = (cy + sp).clamp(0, 65_535);
                    crate::mirror_inject::send(&format!("M 0 {cx} {top}"));
                    crate::mirror_inject::send(&format!("M 1 {cx} {bot}"));
                    last_spread = sp;
                    idle = 0;
                } else {
                    idle += 1;
                    if idle >= IDLE_TICKS {
                        flush_pinch();
                        idle = 0;
                    }
                }
            } else {
                last_x = -1;
                last_y = -1;
                last_spread = i32::MIN;
                idle = 0;
            }
        }
    });
}

/// Attach the navigation pad-probe to glimagesink's sink pad. Navigation events
/// travel UPSTREAM from the sink (which owns the window), so an upstream-event
/// probe on its sink pad sees every mouse/key the user generates in the mirror
/// window — no foreign-window X11 grabbing (that crashed; see git log).
fn attach_input_probe(pipeline: &gst::Pipeline, disp_w: u32, disp_h: u32) {
    let Some(vsink) = pipeline.by_name("vsink") else {
        tracing::warn!("mirror: vsink not found — input control disabled");
        return;
    };
    let Some(pad) = vsink.static_pad("sink") else {
        tracing::warn!("mirror: vsink has no sink pad — input control disabled");
        return;
    };
    POINTER_DOWN.store(false, Ordering::Relaxed);
    pad.add_probe(gst::PadProbeType::EVENT_UPSTREAM, move |_pad, info| {
        if let Some(gst::PadProbeData::Event(ref ev)) = info.data {
            if ev.type_() == gst::EventType::Navigation {
                if let Some(s) = ev.structure() {
                    handle_nav(s, disp_w, disp_h);
                }
            }
        }
        gst::PadProbeReturn::Ok
    });
    tracing::info!("mirror: input control probe attached (mouse/keyboard → phone)");
}

/// True if the box has any DRM render node. We scan for `renderD*` rather than
/// assuming `renderD128`: on a hybrid / multi-GPU machine the usable node can be
/// `renderD129` (or higher), and hardcoding 128 would wrongly fall back to slow
/// software decode. The `vah265dec` element find below is the real gate.
fn has_drm_render_node() -> bool {
    std::fs::read_dir("/dev/dri")
        .map(|rd| {
            rd.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
        })
        .unwrap_or(false)
}

/// Pick the best available HEVC (H.265) decoder backend.
pub fn detect_decoder_backend() -> &'static str {
    let _ = gst::init();
    if std::process::Command::new("nvidia-smi")
        .arg("-L")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
        && gst::ElementFactory::find("nvh265dec").is_some()
    {
        return "nvdec";
    }
    if has_drm_render_node() && gst::ElementFactory::find("vah265dec").is_some() {
        return "vaapi";
    }
    "software"
}

/// Pre-decoder queue: non-leaky 4 buffers (absorbs IDR bursts without dropping
/// reference frames). Post-decoder queue: leaky=downstream 1 buffer (always show
/// the freshest frame). `h265parse config-interval=-1` re-injects VPS/SPS/PPS before
/// every IDR so a viewer that joined mid-stream recovers at the next keyframe.
fn pipeline_string_for_backend(backend: &str, disp_w: u32, disp_h: u32) -> String {
    // Scale the DISPLAYED frame to (disp_w × disp_h) — a portrait size that fits
    // the screen — so glimagesink's auto-created window opens at that aspect.
    // Without this the window is born at the full capture size (e.g. 1080×2340);
    // on a screen shorter than the video (a laptop panel) the WM clamps that
    // oversized window into a landscape shape and the portrait frame gets black
    // PILLARBARS on the sides. Scaling to a fitting portrait size keeps the
    // window the phone's aspect → no bars. Full res is still decoded (crisp);
    // only the on-screen size is bounded. GL path scales on the GPU
    // (glcolorscale); software path uses videoscale.
    match backend {
        "nvdec" => format!(
            "appsrc name=src is-live=true format=time block=true max-bytes=1048576 do-timestamp=true \
             caps=video/x-h265,stream-format=byte-stream,alignment=au ! \
             queue max-size-buffers=4 max-size-bytes=4194304 max-size-time=0 ! \
             h265parse name=parser config-interval=-1 ! \
             nvh265dec ! glupload ! glcolorconvert ! \
             glcolorscale ! video/x-raw(memory:GLMemory),width={disp_w},height={disp_h} ! \
             videorate ! video/x-raw(memory:GLMemory),framerate=60/1 ! \
             queue name=postq max-size-buffers=3 max-size-bytes=0 max-size-time=0 ! \
             glimagesink name=vsink sync=true force-aspect-ratio=true"
        ),
        "vaapi" => format!(
            "appsrc name=src is-live=true format=time block=true max-bytes=1048576 do-timestamp=true \
             caps=video/x-h265,stream-format=byte-stream,alignment=au ! \
             queue max-size-buffers=4 max-size-bytes=4194304 max-size-time=0 ! \
             h265parse name=parser config-interval=-1 ! \
             vah265dec ! videoconvert ! glupload ! glcolorconvert ! \
             glcolorscale ! video/x-raw(memory:GLMemory),width={disp_w},height={disp_h} ! \
             queue name=postq leaky=downstream max-size-buffers=1 max-size-bytes=0 max-size-time=0 ! \
             glimagesink name=vsink sync=false force-aspect-ratio=true"
        ),
        _ => format!(
            "appsrc name=src is-live=true format=time block=true max-bytes=1048576 do-timestamp=true \
             caps=video/x-h265,stream-format=byte-stream,alignment=au ! \
             queue max-size-buffers=4 max-size-bytes=4194304 max-size-time=0 ! \
             h265parse name=parser config-interval=-1 ! \
             avdec_h265 max-threads=4 ! videoconvert ! videoscale ! \
             video/x-raw,width={disp_w},height={disp_h} ! \
             queue name=postq leaky=downstream max-size-buffers=1 max-size-bytes=0 max-size-time=0 ! \
             xvimagesink name=vsink sync=false"
        ),
    }
}

/// Pick an on-screen window size: the phone frame (`vw`×`vh`, portrait) scaled to
/// fit the LARGEST connected monitor — that's the primary/active display the WM
/// opens a new window on, so the mirror comes out big on a big external screen
/// (and merely fits a laptop-only setup). Capped so we never upscale past the
/// decoded resolution; aspect preserved. The window stays resizable — the user
/// can drag it to any size and the aspect is kept. Falls back to the main
/// window's monitor, then a 1080p assumption, if the monitor list is unknown.
fn fit_display_size(app: &AppHandle, vw: u32, vh: u32) -> (u32, u32) {
    use tauri::Manager;
    let monitor_h = app
        .available_monitors()
        .ok()
        .and_then(|ms| ms.iter().map(|m| m.size().height).filter(|&h| h > 0).max())
        .or_else(|| {
            app.get_webview_window("main")
                .and_then(|w| w.current_monitor().ok().flatten())
                .map(|m| m.size().height)
        })
        .filter(|&h| h > 0)
        .unwrap_or(1080);
    // Leave headroom for the title bar / panel so the window isn't clamped.
    // No `.min(vh)`: we WANT to upscale a 720p stream to fill the screen (the GL
    // scaler handles it), otherwise the window shrinks to the decode resolution.
    let max_h = ((monitor_h * 90) / 100).max(2);
    let scale = max_h as f64 / vh as f64;
    let mut w = (vw as f64 * scale).round() as u32;
    let mut h = max_h;
    // Even dimensions (some sinks/scalers dislike odd sizes).
    w &= !1;
    h &= !1;
    (w.max(2), h.max(2))
}

/// Synchronously tear down the input + session side-effects when the mirror
/// window is closed by the user (X button / WM). Without this, closing the GL
/// window stopped only the GStreamer pipeline and LEAKED the uinput injector,
/// the adb forward and the control session.
fn cleanup_session() {
    flush_scroll();
    flush_pinch();
    crate::mirror_inject::stop();
    if let Ok(mut g) = INPUT_TX.lock() {
        *g = None;
    }
    POINTER_DOWN.store(false, Ordering::Relaxed);
    SCROLL_ACTIVE.store(false, Ordering::Relaxed);
    stop_pipeline();
    // Dropping the handle ends the writer/reader loops (its channel senders go
    // away), which closes the phone-side session.
    if let Ok(mut g) = MIRROR_HANDLE.lock() {
        *g = None;
    }
}

/// Set the live pipeline to Null (closing glimagesink's window) and forget it.
/// Idempotent — `take()` means a second caller (bus EOS, UI stop, cleanup) is a
/// no-op.
fn stop_pipeline() {
    if let Some(p) = MIRROR_PIPELINE.lock().ok().and_then(|mut g| g.take()) {
        let _ = p.set_state(gst::State::Null);
    }
}

/// Retitle glimagesink's auto-created GL window (default "OpenGL renderer") to
/// the phone's name AND strip its WM decorations (the fractional-scaling-broken
/// X button). We only change properties on a foreign window — safe, unlike
/// rendering into one (which crashes on XWayland). Best-effort: retries while
/// the window comes up, then gives up.
fn rename_gl_window(title: String) {
    std::thread::spawn(move || {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
        use x11rb::wrapper::ConnectionExt as _;

        let Ok((conn, screen)) = x11rb::connect(None) else { return };
        let root = conn.setup().roots[screen].root;

        fn wm_name(conn: &impl x11rb::connection::Connection, win: u32) -> Option<String> {
            let r = conn
                .get_property(false, win, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
                .ok()?
                .reply()
                .ok()?;
            if r.value.is_empty() {
                return None;
            }
            Some(String::from_utf8_lossy(&r.value).to_string())
        }
        fn find(conn: &impl x11rb::connection::Connection, win: u32, target: &str) -> Option<u32> {
            if wm_name(conn, win).as_deref() == Some(target) {
                return Some(win);
            }
            let tree = conn.query_tree(win).ok()?.reply().ok()?;
            for child in tree.children {
                if let Some(w) = find(conn, child, target) {
                    return Some(w);
                }
            }
            None
        }

        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(100));
            let Some(win) = find(&conn, root, "OpenGL renderer") else { continue };
            // Strip the server-side title bar + close button. Under fractional
            // display scaling the WM's X-button hit-target is misplaced — the
            // user had to click it 8-9× to land it — so we drop the decoration
            // entirely; the mirror is closed from the app's "Stop sharing"
            // button. _MOTIF_WM_HINTS: flags=DECORATIONS(2), decorations=0(none);
            // Mutter applies it live. The window stays movable (Super+drag).
            if let Some(motif) = conn
                .intern_atom(false, b"_MOTIF_WM_HINTS")
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| r.atom)
            {
                let _ = conn.change_property32(
                    PropMode::REPLACE, win, motif, motif, &[2u32, 0, 0, 0, 0],
                );
            }
            let _ = conn.change_property8(
                PropMode::REPLACE,
                win,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                title.as_bytes(),
            );
            let net = conn.intern_atom(false, b"_NET_WM_NAME").ok().and_then(|c| c.reply().ok());
            let utf8 = conn.intern_atom(false, b"UTF8_STRING").ok().and_then(|c| c.reply().ok());
            if let (Some(net), Some(utf8)) = (net, utf8) {
                let _ = conn.change_property8(
                    PropMode::REPLACE,
                    win,
                    net.atom,
                    utf8.atom,
                    title.as_bytes(),
                );
            }
            let _ = conn.flush();
            return;
        }
    });
}

/// Build + start the GStreamer pipeline; returns `(pipeline, appsrc)`. A bus
/// thread tears the pipeline down on EOS/Error (and treats the user closing the
/// window as a clean stop).
fn spawn_gstreamer_player(
    app: &AppHandle,
    backend: &str,
    vw: u32,
    vh: u32,
) -> Result<(gst::Pipeline, gst_app::AppSrc), String> {
    // Force GL windowing onto X11/GLX even on a Wayland session (XWayland).
    std::env::set_var("GST_GL_WINDOW", "x11");
    std::env::set_var("GST_GL_API", "opengl");
    gst::init().map_err(|e| format!("gst init: {e}"))?;

    let (disp_w, disp_h) = fit_display_size(app, vw, vh);
    tracing::info!(vw, vh, disp_w, disp_h, "mirror: display window fit");
    let pipeline_str = pipeline_string_for_backend(backend, disp_w, disp_h);
    let element = gst::parse::launch(&pipeline_str).map_err(|e| format!("gst parse: {e}"))?;
    let pipeline = element
        .downcast::<gst::Pipeline>()
        .map_err(|_| "pipeline downcast".to_string())?;
    let appsrc = pipeline
        .by_name("src")
        .ok_or_else(|| "appsrc not found".to_string())?
        .downcast::<gst_app::AppSrc>()
        .map_err(|_| "appsrc cast".to_string())?;
    appsrc.set_is_live(true);
    appsrc.set_do_timestamp(true);
    appsrc.set_block(false);
    appsrc.set_format(gst::Format::Time);
    appsrc.set_max_bytes(4 * 1024 * 1024);

    // Wire laptop→phone input: mouse/keyboard in this window → sealed packets.
    attach_input_probe(&pipeline, disp_w, disp_h);

    // Retitle the GL window from "OpenGL renderer" to the phone's name.
    let title = MIRROR_TITLE
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| "Phone".to_string());
    rename_gl_window(title);

    let bus = pipeline.bus().ok_or_else(|| "gst bus unavailable".to_string())?;
    let app_bus = app.clone();
    let pipeline_bus = pipeline.clone();
    std::thread::spawn(move || {
        for msg in bus.iter_timed(gst::ClockTime::NONE) {
            use gst::MessageView;
            match msg.view() {
                MessageView::Error(err) => {
                    let s = err.error().to_string();
                    let m = if s.contains("Quit requested") {
                        "mirror window closed by user".to_string()
                    } else {
                        format!("gst error: {} ({})", err.error(), err.debug().unwrap_or_default())
                    };
                    tracing::warn!("mirror: bus ERROR → {m} — tearing down");
                    let _ = app_bus.emit("mirror-player", serde_json::json!({ "message": m }));
                    let _ = pipeline_bus.set_state(gst::State::Null);
                    cleanup_session();
                    break;
                }
                MessageView::Eos(..) => {
                    tracing::warn!("mirror: bus EOS — tearing down");
                    let _ = app_bus.emit("mirror-player", serde_json::json!({ "message": "gst EOS" }));
                    let _ = pipeline_bus.set_state(gst::State::Null);
                    cleanup_session();
                    break;
                }
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| format!("set Playing: {e:?}"))?;
    // Publish the pipeline so a UI "Stop sharing" / cleanup can close the window
    // directly (the window's own X is unreliable under fractional scaling).
    if let Ok(mut g) = MIRROR_PIPELINE.lock() {
        *g = Some(pipeline.clone());
    }
    Ok((pipeline, appsrc))
}

/// Spawn the player task: build the pipeline and pump access units from `au_rx`
/// into `appsrc` until the channel closes (session ended) or the pipeline dies.
fn start_player(
    app: AppHandle,
    backend: &'static str,
    vw: u32,
    vh: u32,
    mut au_rx: mpsc::Receiver<Vec<u8>>,
) {
    tracing::info!(backend, "mirror: starting GStreamer player");
    tokio::spawn(async move {
        let (pipeline, appsrc) = match spawn_gstreamer_player(&app, backend, vw, vh) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("mirror: GStreamer start FAILED: {e}");
                let _ = app.emit("mirror-player", serde_json::json!({ "message": format!("gst start failed: {e}") }));
                return;
            }
        };
        tracing::info!("mirror: GStreamer pipeline Playing — window should open");
        let _ = app.emit("mirror-player", serde_json::json!({ "message": "mirror window opening" }));
        let mut first = true;
        while let Some(au) = au_rx.recv().await {
            if first {
                first = false;
                tracing::info!(bytes = au.len(), "mirror: first AU → appsrc");
            }
            let mut buffer = match gst::Buffer::with_size(au.len()) {
                Ok(b) => b,
                Err(_) => break,
            };
            if let Some(bm) = buffer.get_mut() {
                if let Ok(mut map) = bm.map_writable() {
                    map.as_mut_slice().copy_from_slice(&au);
                }
            }
            if appsrc.push_buffer(buffer).is_err() {
                break;
            }
        }
        let _ = appsrc.end_of_stream();
        let _ = pipeline.set_state(gst::State::Null);
        let _ = app.emit("mirror-player", serde_json::json!({ "message": "mirror stopped" }));
    });
}

/// Orchestrate a full laptop-side mirror session:
///  1. bind a local UDP socket for the sealed video,
///  2. open the dedicated Noise-IK control session to the phone (`start_mirror_
///     session`) carrying the START params (incl. our UDP port),
///  3. derive the UDP media key from the IK handshake hash,
///  4. run the UDP receiver (decrypt + reassemble → access units),
///  5. feed the GStreamer player.
///
/// The peer IP + identity/PRS come from the worker (LAN discovery). Returns once
/// the session is established (streaming proceeds in spawned tasks).
#[allow(clippy::too_many_arguments)]
pub async fn spawn_mirror(
    app: AppHandle,
    phone_addr: SocketAddr,
    static_priv: &X25519SecBytes,
    peer_pub: &[u8; 32],
    prs: &[u8; 32],
    local_counter: u64,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
) -> Result<(), String> {
    // Bind via socket2 so we can grow SO_RCVBUF before handing off to tokio: the
    // phone fires all fragments of a frame back-to-back, so a whole IDR (tens of
    // KB → tens of packets) lands as one burst. A small default buffer drops the
    // tail of each burst → incomplete frames. 4 MB (kernel rmem_max) holds many
    // frames' worth, killing the burst loss.
    // Open the control session (START/STOP/input + keepalive). Video no longer
    // rides UDP, so `udp_port` is vestigial — the phone serves video on its own
    // fixed TCP port and the laptop connects out to it.
    let start = MirrorStart { w: width, h: height, fps, bitrate, udp_port: 0 };
    let handle =
        start_mirror_session(phone_addr, static_priv, peer_pub, prs, local_counter, start).await?;

    // TCP video data-plane (reliable + ordered, like scrcpy): connect OUT to the
    // phone's video server and stream sealed access units. TCP never loses a
    // frame, so there are no UDP-style burst-loss freezes — and no hole-punch is
    // needed (the laptop is the connecting side; its firewall allows that).
    let key = mirror_udp::derive_media_key(&handle.handshake_hash);
    let (au_tx, au_rx) = mpsc::channel::<Vec<u8>>(8);
    tokio::spawn(mirror_tcp::run_tcp_video_receiver(phone_addr.ip(), key, au_tx));

    let backend = detect_decoder_backend();
    start_player(app, backend, width, height, au_rx);

    // Bring up the real-touch injector (scrcpy-style uinput over adb) off the
    // async runtime — push + connect-retries take ~1-2s and must not block a
    // tokio worker. Until it's up, input rides the accessibility fallback.
    std::thread::spawn(|| {
        crate::mirror_inject::start();
    });

    // Publish the input channel BEFORE storing the handle so the navigation
    // probe can start pushing the moment the window accepts events.
    if let Ok(mut g) = INPUT_TX.lock() {
        *g = Some(handle.input_tx.clone());
    }
    // Keep the control session alive for the stream's lifetime (the input probe
    // reaches its `input_tx` via INPUT_TX; `stop_mirror` closes it).
    if let Ok(mut g) = MIRROR_HANDLE.lock() {
        *g = Some(handle);
    }
    Ok(())
}

/// Tear down the active mirror session (sends STOP to the phone + drops the
/// session, which ends the writer/reader loops and the UDP receiver).
pub async fn stop_mirror() {
    flush_scroll();
    flush_pinch();
    crate::mirror_inject::stop();
    if let Ok(mut g) = INPUT_TX.lock() {
        *g = None;
    }
    POINTER_DOWN.store(false, Ordering::Relaxed);
    SCROLL_ACTIVE.store(false, Ordering::Relaxed);
    // Close the native window + decoder now (a UI "Stop sharing" must work even
    // when the window's X button is dead under fractional scaling).
    stop_pipeline();
    let handle = MIRROR_HANDLE.lock().ok().and_then(|mut g| g.take());
    if let Some(h) = handle {
        h.stop().await;
    }
}

/// `UiCmd::StartMirror` handler — resolve the phone's CURRENT LAN address and
/// open a mirror session. Split out of `run_worker`.
///
/// Address resolution is `lan::resolve_peer_addr`, NOT a raw read of the
/// cached IP: the cache goes stale whenever the phone's DHCP lease moves
/// (Wi-Fi rejoin, router restart), and blindly dialing it made the mirror
/// "sometimes just not start" on the very same network. The resolver probes
/// the cached IP first (2 s), falls back to a fresh mDNS browse, then the
/// probed gateway (hotspot) — and if the session STILL fails on an address
/// that answered TCP, we force one fresh rediscovery and retry before giving
/// up, emitting a `mirror-player` message either way so the UI never fails
/// silently.
pub(crate) async fn handle_start_cmd(
    ctx: &crate::worker_ctx::WorkerCtx,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
) {
    // Tear down any prior session first so repeated clicks don't stack up
    // receivers / leave a half-open phone server.
    stop_mirror().await;
    let peer = ctx.peer_store.list().ok().and_then(|l| l.into_iter().next());
    let Some(peer) = peer else {
        tracing::warn!("start mirror: no trusted peer");
        return;
    };
    if let Ok(mut g) = MIRROR_TITLE.lock() {
        *g = Some(peer.peer_name.clone().unwrap_or_else(|| "Phone".to_string()));
    }
    let counter = ctx.peer_store.load_counter(&peer.peer_static_pub).unwrap_or(0);
    let app_c = ctx.app.clone();
    let identity_c = ctx.identity.clone();
    tokio::spawn(async move {
        let mut tried: Option<SocketAddr> = None;
        let mut last_err: Option<String> = None;
        for fresh in [false, true] {
            let Some(addr) = crate::lan::resolve_peer_addr(fresh).await else {
                break;
            };
            // The retry pass only helps on a DIFFERENT address — re-dialing
            // the one that just failed would just fail again.
            if tried == Some(addr) {
                continue;
            }
            tried = Some(addr);
            tracing::info!(%addr, fresh, "start mirror → opening session");
            match spawn_mirror(
                app_c.clone(),
                addr,
                &identity_c.static_priv.0,
                &peer.peer_static_pub,
                &peer.prs,
                counter,
                width,
                height,
                fps,
                bitrate,
            )
            .await
            {
                Ok(()) => return,
                Err(e) => {
                    // An address can pass the TCP probe yet fail the session —
                    // e.g. the old lease now belongs to another device. Round 2
                    // skips the cache and rediscovers from scratch.
                    tracing::warn!(%addr, "start mirror failed: {e}{}", if fresh { "" } else { " — rediscovering + retrying" });
                    last_err = Some(e);
                }
            }
        }
        let msg = match last_err {
            Some(e) => format!("mirror failed: {e}"),
            None => "phone not reachable on LAN".to_string(),
        };
        tracing::warn!("start mirror: {msg}");
        let _ = app_c.emit("mirror-player", serde_json::json!({ "message": msg }));
    });
}

/// `UiCmd::StopMirror` handler — fire-and-forget teardown.
pub(crate) fn handle_stop_cmd() {
    tokio::spawn(async {
        stop_mirror().await;
    });
}
