//! A real extra monitor for the session, created on demand — the display half
//! of "use the phone as a second screen".
//!
//! This is NOT a picture of an existing screen. `RecordVirtual` asks Mutter to
//! add a monitor to the running session, so the shell treats it like any other
//! output: it shows up in Displays, windows can be dragged onto it, and the
//! pointer walks onto it by itself. That last part matters — none of
//! [`crate::universal_control`]'s edge-crossing machinery is needed here,
//! because there is no edge to cross.
//!
//! The catch is that `org.gnome.Mutter.ScreenCast` is GNOME's own D-Bus API,
//! not a portal, so this path is GNOME-only. The standard ScreenCast portal
//! cannot do it: it hands out a view of a monitor that already exists.
//!
//! Mutter materialises the monitor lazily — it appears only once something
//! consumes the PipeWire stream and the format negotiation settles, which is
//! what [`crate::laptop_cast`]'s pipeline does. It disappears when [`stop`] is
//! called or the D-Bus connection drops, which is why the connection is kept
//! inside the handle rather than borrowed.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use zbus::zvariant::{OwnedObjectPath, Value};

const BUS: &str = "org.gnome.Mutter.ScreenCast";
const PATH: &str = "/org/gnome/Mutter/ScreenCast";

/// Mutter's remote-input API — the same one gnome-remote-desktop drives.
///
/// Pairing it with the ScreenCast session is what makes the virtual monitor
/// *interactive* rather than a picture: input notified through this session is
/// delivered to the compositor, and `NotifyPointerMotionAbsolute` takes the
/// STREAM as its first argument, so coordinates are in that monitor's own
/// space. Without the pairing there is no stream to address and absolute
/// positioning has no frame of reference.
///
/// GNOME-only, exactly as the ScreenCast half already is. Verified present on
/// this machine (mutter-18 / GNOME 50): NotifyPointerMotionAbsolute,
/// NotifyPointerButton, NotifyPointerAxis(Discrete), NotifyTouchDown/Motion/Up.
const RD_BUS: &str = "org.gnome.Mutter.RemoteDesktop";
const RD_PATH: &str = "/org/gnome/Mutter/RemoteDesktop";

/// Pointer artwork drawn into the extend stream. Small enough to embed.
const CURSOR_PNG: &[u8] = include_bytes!("../assets/cursor.png");

/// Put the cursor bitmap somewhere `gdkpixbufoverlay` can load it from.
/// Rewritten every time so an older build's artwork can't linger.
pub(crate) fn stage_cursor_image() -> Option<std::path::PathBuf> {
    let p = std::env::temp_dir().join("vortex-cursor.png");
    std::fs::write(&p, CURSOR_PNG).ok()?;
    Some(p)
}

/// Keep `overlay` sitting under the real pointer for as long as `alive` holds.
///
/// Mutter will not composite a cursor into a virtual monitor's stream (doing so
/// kills the session outright — see [`create`]), so extend mode draws its own.
/// The position comes from our GNOME extension: it reads `global.get_pointer()`
/// in the shell's own logical coordinates, which is the only place that answer
/// is available without unpicking XWayland's fractional scaling by hand.
///
/// No extension, no cursor — the stream is still perfectly usable, so this
/// degrades quietly rather than failing the cast.
pub(crate) fn spawn_cursor_overlay(
    overlay: gstreamer::Element,
    alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use gstreamer::prelude::*;
    use std::sync::atomic::Ordering;

    tokio::spawn(async move {
        let Ok(conn) = zbus::Connection::session().await else {
            return;
        };
        let shell = match zbus::Proxy::new(
            &conn,
            "org.vortex.Shell",
            "/org/vortex/Shell",
            "org.vortex.Shell1",
        )
        .await
        {
            Ok(p) => p,
            Err(_) => {
                tracing::info!(
                    "virtual-display: shell pointer service absent — no cursor on the phone \
                     (the GNOME extension needs a re-login)"
                );
                return;
            }
        };

        // Matches the stream's 30 fps: polling faster would only make the shell
        // answer questions whose answers never reach a frame.
        let mut tick = tokio::time::interval(Duration::from_millis(33));
        let mut shown = false;
        while alive.load(Ordering::Relaxed) {
            tick.tick().await;
            let Ok((x, y, on)) = shell
                .call::<_, _, (i32, i32, bool)>("GetVirtualPointer", &())
                .await
            else {
                continue;
            };
            if on {
                overlay.set_property("offset-x", x);
                overlay.set_property("offset-y", y);
            }
            if on != shown {
                overlay.set_property("alpha", if on { 1.0f64 } else { 0.0f64 });
                shown = on;
            }
        }
        overlay.set_property("alpha", 0.0f64);
    });
}

/// A live virtual monitor. Dropping it, or calling [`VirtualMonitor::stop`],
/// takes the monitor away again and returns any windows on it to the real
/// screens.
pub(crate) struct VirtualMonitor {
    conn: zbus::Connection,
    session: OwnedObjectPath,
    /// The RemoteDesktop session the ScreenCast session is bound to, and the
    /// stream path to address input at. Both are needed for every notify, and
    /// both die with the session, so they live together here.
    rd_session: OwnedObjectPath,
    stream: OwnedObjectPath,
    /// The PipeWire node carrying this monitor's contents.
    pub(crate) node_id: u32,
}

/// Ask Mutter for a new monitor and return the PipeWire node to read it from.
///
/// The size is not requested here: Mutter negotiates it with whoever consumes
/// the stream, so the caller's caps decide how big the monitor turns out to be.
pub(crate) async fn create() -> Result<VirtualMonitor, String> {
    let conn = zbus::Connection::session()
        .await
        .map_err(|e| format!("session bus: {e}"))?;

    let screencast = zbus::Proxy::new(&conn, BUS, PATH, BUS)
        .await
        .map_err(|e| format!("mutter screencast unavailable (not GNOME?): {e}"))?;
    // The RemoteDesktop session comes FIRST, because the ScreenCast session is
    // created already knowing which one it belongs to — the binding is a
    // property passed at creation, not something that can be attached after.
    //
    // Mutter ties the session's lifetime to the D-Bus CONNECTION that created
    // it: the session is destroyed the moment that connection goes away.
    // Measured while working this out — creating one from `busctl` left an
    // object path that introspected to nothing, because busctl had already
    // exited. So it must be made on `conn`, which the VirtualMonitor holds.
    let remote_desktop = zbus::Proxy::new(&conn, RD_BUS, RD_PATH, RD_BUS)
        .await
        .map_err(|e| format!("mutter remote-desktop unavailable (not GNOME?): {e}"))?;
    let rd_session: OwnedObjectPath = remote_desktop
        .call("CreateSession", &())
        .await
        .map_err(|e| format!("RemoteDesktop CreateSession: {e}"))?;
    let rd_session_proxy = zbus::Proxy::new(
        &conn,
        RD_BUS,
        rd_session.clone(),
        format!("{RD_BUS}.Session"),
    )
    .await
    .map_err(|e| format!("remote-desktop session proxy: {e}"))?;
    let rd_id: String = rd_session_proxy
        .get_property("SessionId")
        .await
        .map_err(|e| format!("remote-desktop SessionId: {e}"))?;

    let mut session_props: HashMap<&str, Value> = HashMap::new();
    session_props.insert("remote-desktop-session-id", Value::from(rd_id.clone()));
    let session: OwnedObjectPath = screencast
        .call("CreateSession", &(session_props,))
        .await
        .map_err(|e| format!("CreateSession: {e}"))?;

    let session_proxy = zbus::Proxy::new(&conn, BUS, session.clone(), format!("{BUS}.Session"))
        .await
        .map_err(|e| format!("session proxy: {e}"))?;

    let mut props: HashMap<&str, Value> = HashMap::new();
    // Cursor as METADATA (2), never composited into the frames (1).
    //
    // Asking Mutter to draw the pointer into a virtual monitor's stream makes it
    // tear the session down after somewhere between 30 and 75 seconds — measured
    // repeatedly, from two different D-Bus clients, and it takes the monitor and
    // the capture with it. Modes 0 and 2 both ran for minutes without a wobble.
    //
    // Metadata rather than hidden because the position still reaches us: when
    // the pipeline learns to composite it, the pointer becomes visible with no
    // change here. Until then the phone shows the screen without a cursor.
    props.insert("cursor-mode", Value::U32(2));
    let stream: OwnedObjectPath = session_proxy
        .call("RecordVirtual", &(props,))
        .await
        .map_err(|e| format!("RecordVirtual: {e}"))?;

    let stream_proxy = zbus::Proxy::new(&conn, BUS, stream.clone(), format!("{BUS}.Stream"))
        .await
        .map_err(|e| format!("stream proxy: {e}"))?;
    // Subscribe BEFORE Start, or the node id can be announced before we listen.
    let mut added = stream_proxy
        .receive_signal("PipeWireStreamAdded")
        .await
        .map_err(|e| format!("subscribe: {e}"))?;

    // ONLY the RemoteDesktop session is started, and the ScreenCast stream
    // comes up with it. Starting the ScreenCast session directly fails once
    // the two are paired — Mutter answers "Must be started from remote desktop
    // session", because the pairing makes the remote-desktop session the owner.
    rd_session_proxy
        .call::<_, _, ()>("Start", &())
        .await
        .map_err(|e| format!("RemoteDesktop Start: {e}"))?;

    let msg = tokio::time::timeout(Duration::from_secs(5), added.next())
        .await
        .map_err(|_| "timed out waiting for the PipeWire node".to_string())?
        .ok_or_else(|| "stream closed before announcing a node".to_string())?;
    let node_id: u32 = msg
        .body()
        .deserialize()
        .map_err(|e| format!("node id: {e}"))?;

    // Mutter tears the session down for reasons of its own — a monitor
    // reconfiguration, a lost client, a compositor hiccup — and the first we
    // would otherwise know about it is the capture pipeline failing a second
    // later with a generic "internal data stream error". Say so plainly.
    if let Ok(mut closed) = session_proxy.receive_signal("Closed").await {
        tokio::spawn(async move {
            if closed.next().await.is_some() {
                tracing::warn!("virtual-display: MUTTER closed the session (not us)");
            }
        });
    }

    tracing::info!(node_id, "virtual-display: monitor session up");
    Ok(VirtualMonitor {
        conn,
        session,
        rd_session,
        stream,
        node_id,
    })
}

impl VirtualMonitor {
    /// A proxy on the paired RemoteDesktop session.
    ///
    /// Rebuilt per call rather than stored: a zbus `Proxy` is a thin handle
    /// over the connection we already hold, and keeping one in the struct
    /// would make `VirtualMonitor` borrow-bound in ways the cast path does not
    /// want. Input arrives in bursts of a few events, not thousands a second —
    /// the gesture stream from a phone is paced by a finger.
    async fn rd(&self) -> Option<zbus::Proxy<'_>> {
        zbus::Proxy::new(
            &self.conn,
            RD_BUS,
            self.rd_session.clone(),
            format!("{RD_BUS}.Session"),
        )
        .await
        .ok()
    }

    /// Move the pointer to a point ON THIS MONITOR.
    ///
    /// `x`/`y` are in the virtual monitor's own pixels, which is the whole
    /// reason the sessions are paired: the stream argument gives Mutter the
    /// frame of reference, so the phone can send a position from the picture
    /// it is showing without knowing anything about the laptop's real display
    /// layout.
    pub(crate) async fn pointer_to(&self, x: f64, y: f64) {
        let Some(rd) = self.rd().await else { return };
        if let Err(e) = rd
            .call::<_, _, ()>(
                "NotifyPointerMotionAbsolute",
                &(self.stream.as_str(), x, y),
            )
            .await
        {
            tracing::debug!("virtual-display: pointer move: {e}");
        }
    }

    /// Press or release a mouse button. `button` is an evdev code (BTN_LEFT is
    /// 0x110), which is what Mutter expects here — not an index.
    pub(crate) async fn pointer_button(&self, button: i32, pressed: bool) {
        let Some(rd) = self.rd().await else { return };
        if let Err(e) = rd
            .call::<_, _, ()>("NotifyPointerButton", &(button, pressed))
            .await
        {
            tracing::debug!("virtual-display: pointer button: {e}");
        }
    }

    /// Scroll by whole notches. Discrete rather than smooth: a phone's fling
    /// arrives as a gesture the sender has already turned into steps, and
    /// feeding smooth deltas would double up the acceleration.
    pub(crate) async fn scroll(&self, dx: i32, dy: i32) {
        let Some(rd) = self.rd().await else { return };
        if let Err(e) = rd
            .call::<_, _, ()>("NotifyPointerAxisDiscrete", &(0u32, dx))
            .await
        {
            tracing::debug!("virtual-display: scroll x: {e}");
        }
        if let Err(e) = rd
            .call::<_, _, ()>("NotifyPointerAxisDiscrete", &(1u32, dy))
            .await
        {
            tracing::debug!("virtual-display: scroll y: {e}");
        }
    }

    /// Take the monitor away. Safe to call more than once.
    pub(crate) async fn stop(&self) {
        let Ok(proxy) = zbus::Proxy::new(
            &self.conn,
            BUS,
            self.session.clone(),
            format!("{BUS}.Session"),
        )
        .await
        else {
            return;
        };
        if let Err(e) = proxy.call::<_, _, ()>("Stop", &()).await {
            tracing::debug!("virtual-display: stop: {e}");
        }
        tracing::info!("virtual-display: monitor removed");
    }
}
