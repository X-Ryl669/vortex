//! Laptop → phone REAL touch+key injection (scrcpy-style), bypassing MIUI's
//! `INJECT_EVENTS` block via a shell-UID uinput helper.
//!
//! `vortex_inject` (a tiny arm64 native binary, embedded below) is pushed to the
//! phone over adb and launched as the **shell** user, where it creates a virtual
//! multi-touch touchscreen + keyboard via `/dev/uinput`. The laptop streams
//! `D/M/U/E/K` commands to it over an **abstract unix socket** tunneled by
//! `adb forward tcp:<port> localabstract:vortex_inject`. A socket (not adb-shell
//! stdin, which batches) is what keeps the stream low-latency and un-stuttered —
//! the same reason scrcpy uses one.
//!
//! Real `MotionEvent`s/`KeyEvent`s mean low latency, true multitouch and a real
//! keyboard — the only way to reach first-class control on a non-rooted MIUI
//! device that blocks the InputManager / `adb shell input` path. When adb is
//! unavailable, [`start`] returns false and the caller falls back to the
//! AccessibilityService path.

use std::io::Write;
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

/// The embedded arm64 injector binary (built from `a3/inject/vortex_inject.c`).
const BINARY: &[u8] = include_bytes!("../assets/vortex_inject");
const REMOTE_PATH: &str = "/data/local/tmp/vortex_inject";
const SOCKET_NAME: &str = "localabstract:vortex_inject";
/// Default host-side TCP port the adb tunnel forwards to the device socket.
/// Only a seed — [`start`] asks adb to allocate a free port (`tcp:0`) and
/// stores the real one in [`ACTIVE_PORT`], so a busy 28250 can't knock control
/// out to the accessibility fallback.
const LOCAL_PORT: u16 = 28250;

/// The host port adb actually bound for this session (set in [`start`]).
static ACTIVE_PORT: AtomicU16 = AtomicU16::new(LOCAL_PORT);

static INJECT: Mutex<Option<Injector>> = Mutex::new(None);

/// Set while `stop()` is tearing the session down, so the writer's self-heal
/// (which fires on a socket error) doesn't race it and re-launch the injector
/// just after the user closed the mirror.
static STOPPING: AtomicBool = AtomicBool::new(false);

enum Cmd {
    Line(String),
    Quit,
}

struct Injector {
    child: Child,
    /// Commands are queued here (never blocks the caller) and drained by a
    /// dedicated socket-writer thread — so the GStreamer navigation thread can
    /// NEVER block on socket I/O (which froze the whole pipeline).
    tx: Sender<Cmd>,
    worker: Option<JoinHandle<()>>,
}

fn adb(args: &[&str]) -> bool {
    match Command::new("adb").args(args).output() {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            // Surface WHY (device offline/unauthorized, "Text file busy", no
            // device…) — this used to fail silently and drop to accessibility.
            tracing::debug!(
                ?args,
                "adb command failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        Err(e) => {
            tracing::debug!(?args, "adb not runnable: {e}");
            false
        }
    }
}

/// Like [`adb`] but returns trimmed stdout on success — used for
/// `adb forward tcp:0 …`, which prints the host port adb allocated.
fn adb_capture(args: &[&str]) -> Option<String> {
    let out = Command::new("adb").args(args).output().ok()?;
    if !out.status.success() {
        tracing::debug!(?args, "adb (capture) failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Push the embedded injector to the phone (overwrites any stale copy) and make
/// it executable. Returns false if adb isn't available / there's no device.
fn ensure_pushed() -> bool {
    let tmp = std::env::temp_dir().join("vortex_inject");
    if std::fs::write(&tmp, BINARY).is_err() {
        tracing::warn!("mirror inject: couldn't stage injector to temp");
        return false;
    }
    let Some(tmp_str) = tmp.to_str() else { return false };
    // Kill any still-running injector FIRST and drop the old file: a live copy
    // holds /data/local/tmp/vortex_inject open, so `adb push` fails with "Text
    // file busy" — which silently dropped control to the accessibility fallback
    // (the user's "can't control" / "froze"). Doing this before the push fixes
    // it; the forward/launch below re-creates a clean injector.
    let _ = adb(&["shell", "pkill", "-f", "vortex_inject"]);
    let _ = adb(&["shell", "rm", "-f", REMOTE_PATH]);
    if !adb(&["push", tmp_str, REMOTE_PATH]) {
        tracing::warn!("mirror inject: adb push failed (device offline / unauthorized?)");
        return false;
    }
    adb(&["shell", "chmod", "755", REMOTE_PATH]);
    true
}

/// Start the real-touch injector for a mirror session. Replaces any prior one.
/// Returns false (→ accessibility fallback) when adb/device is unavailable.
pub fn start() -> bool {
    stop();
    // stop() raised STOPPING while tearing down; clear it now that we're
    // (re)starting so the new writer's self-heal can run.
    STOPPING.store(false, Ordering::SeqCst);
    // ensure_pushed() already pkills any stale injector before pushing.
    if !ensure_pushed() {
        tracing::warn!("mirror inject: adb push failed — using accessibility fallback");
        return false;
    }
    // Tunnel the device's abstract socket to a host TCP port. Remove any stale
    // forward we created last time, then let adb allocate a FREE port (tcp:0 →
    // it prints the chosen port) instead of a fixed 28250 that might be taken.
    let _ = adb(&["forward", "--remove", &format!("tcp:{}", ACTIVE_PORT.load(Ordering::SeqCst))]);
    let port = match adb_capture(&["forward", "tcp:0", SOCKET_NAME])
        .and_then(|s| s.parse::<u16>().ok())
    {
        Some(p) => p,
        None => {
            tracing::warn!("mirror inject: adb forward failed — accessibility fallback");
            return false;
        }
    };
    ACTIVE_PORT.store(port, Ordering::SeqCst);
    // Launch the injector (it binds the abstract socket and listens).
    let spawn = Command::new("adb")
        .args(["shell", REMOTE_PATH])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let child = match spawn {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("mirror inject: spawn failed: {e} — accessibility fallback");
            return false;
        }
    };
    let Some(stream) = connect_socket() else {
        tracing::warn!("mirror inject: socket connect failed — accessibility fallback");
        let mut child = child;
        let _ = child.kill();
        return false;
    };
    // Dedicated writer thread: owns the socket, drains the queue. The producer
    // (GStreamer thread) only ever does a non-blocking channel send.
    let (tx, rx) = mpsc::channel::<Cmd>();
    let worker = std::thread::spawn(move || writer_loop(stream, rx));
    if let Ok(mut g) = INJECT.lock() {
        *g = Some(Injector { child, tx, worker: Some(worker) });
    }
    tracing::info!("mirror inject: uinput injector connected (real-touch, scrcpy-style)");
    true
}

/// Connect to the adb-forwarded injector socket, retrying while the device-side
/// helper finishes binding (it needs ~700ms to set up the uinput device).
fn connect_socket() -> Option<TcpStream> {
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(80));
        if let Ok(s) = TcpStream::connect(("127.0.0.1", ACTIVE_PORT.load(Ordering::SeqCst))) {
            let _ = s.set_nodelay(true);
            return Some(s);
        }
    }
    None
}

fn write_line(stream: &mut TcpStream, line: &str) -> bool {
    stream
        .write_all(line.as_bytes())
        .and_then(|_| stream.write_all(b"\n"))
        .is_ok()
}

/// Drain the command queue onto the socket. Self-heals a dropped adb tunnel:
/// on a write error it first tries to RECONNECT the socket (the device helper
/// usually outlives a brief TCP hiccup, replaying the lost line); if that keeps
/// failing the injector is presumed dead, so we clear the global — input then
/// falls back to the AccessibilityService control plane instead of freezing —
/// and kick ONE background re-establish to win real-touch back. Exits cleanly
/// on `Quit` or when the queue's sender is dropped (stopped / replaced).
fn writer_loop(mut stream: TcpStream, rx: Receiver<Cmd>) {
    loop {
        let line = match rx.recv() {
            Ok(Cmd::Line(l)) => l,
            Ok(Cmd::Quit) => {
                let _ = stream.write_all(b"Q\n");
                let _ = stream.flush();
                return;
            }
            Err(_) => return, // sender dropped — session stopped or replaced
        };
        if write_line(&mut stream, &line) {
            continue;
        }
        // Write failed — try to reconnect the adb tunnel for ~2s.
        let mut recovered = false;
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(100));
            if let Ok(s) = TcpStream::connect(("127.0.0.1", ACTIVE_PORT.load(Ordering::SeqCst))) {
                let _ = s.set_nodelay(true);
                stream = s;
                let _ = write_line(&mut stream, &line); // replay the lost line
                recovered = true;
                break;
            }
        }
        if recovered {
            tracing::info!("mirror inject: control socket reconnected");
            continue;
        }
        tracing::warn!("mirror inject: control socket lost — accessibility fallback + re-establish");
        on_injector_lost();
        return;
    }
}

/// The writer's socket died unrecoverably mid-session. Clear the injector so
/// `active()` flips false (→ accessibility control, no freeze), then kick ONE
/// delayed re-establish so real-touch returns if the device is still reachable.
/// No-op re-launch if a concurrent `stop()` is tearing down.
fn on_injector_lost() {
    if let Some(mut inj) = INJECT.lock().ok().and_then(|mut g| g.take()) {
        // Our own worker handle lives in here — detach (never join ourselves),
        // then reap the dead adb child.
        inj.worker = None;
        let _ = inj.child.kill();
        let _ = inj.child.wait();
    }
    let _ = adb(&["forward", "--remove", &format!("tcp:{}", ACTIVE_PORT.load(Ordering::SeqCst))]);
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(1));
        if !STOPPING.load(Ordering::SeqCst) && !active() {
            let _ = start(); // best-effort; leaves accessibility in charge if adb is gone
        }
    });
}

/// True when the real-touch injector is live, so input capture routes here
/// instead of the AccessibilityService control plane.
pub fn active() -> bool {
    INJECT.lock().map(|g| g.is_some()).unwrap_or(false)
}

/// Send one protocol line (`D slot nx ny`, `M slot nx ny`, `U slot`,
/// `E keycode val`, `K back`…). Best-effort: enqueue only — the writer thread
/// owns the socket and self-heals a dropped tunnel.
pub fn send(line: &str) {
    if let Ok(g) = INJECT.lock() {
        if let Some(inj) = g.as_ref() {
            // Non-blocking: just enqueue. The writer thread does the socket I/O.
            let _ = inj.tx.send(Cmd::Line(line.to_string()));
        }
    }
}

/// Tear down the injector (sends `Q`, stops the writer thread, kills the adb
/// process — which destroys the virtual device — and removes the forward).
pub fn stop() {
    // Block the writer's self-heal from re-launching us during teardown.
    STOPPING.store(true, Ordering::SeqCst);
    let taken = INJECT.lock().ok().and_then(|mut g| g.take());
    if let Some(mut inj) = taken {
        let _ = inj.tx.send(Cmd::Quit);
        if let Some(w) = inj.worker.take() {
            let _ = w.join();
        }
        let _ = inj.child.kill();
        let _ = inj.child.wait();
    }
    let _ = adb(&["forward", "--remove", &format!("tcp:{}", ACTIVE_PORT.load(Ordering::SeqCst))]);
}
