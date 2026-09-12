//! Spike: does a virtual monitor paired with a RemoteDesktop session accept
//! input?
//!
//! The phone can already be a real second monitor for the laptop, but only to
//! look at. Making it interactive rests on one claim: that Mutter's private
//! `org.gnome.Mutter.RemoteDesktop` will deliver input addressed at a
//! ScreenCast STREAM, so a touch on the phone lands at the right place on that
//! monitor. This proves or disproves it with nothing else in the way.
//!
//! Self-contained on purpose — it speaks D-Bus directly rather than going
//! through the app's `virtual_display` module, so a pass means the SEQUENCE is
//! right and not that our wrapper happens to work.
//!
//! Run:  cargo run --features dev-tools --bin vortex-vd-input-test
//! Pass: a new display appears, the cursor jumps onto it and walks across.
//!
//! GNOME only. Verified present on mutter-18 / GNOME 50:
//! NotifyPointerMotionAbsolute, NotifyPointerButton, NotifyPointerAxisDiscrete,
//! NotifyTouchDown/Motion/Up, and `remote-desktop-session-id` as the property
//! that binds the two sessions.

use std::collections::HashMap;
use std::time::Duration;

use zbus::zvariant::{OwnedObjectPath, Value};

const SC_BUS: &str = "org.gnome.Mutter.ScreenCast";
const SC_PATH: &str = "/org/gnome/Mutter/ScreenCast";
const RD_BUS: &str = "org.gnome.Mutter.RemoteDesktop";
const RD_PATH: &str = "/org/gnome/Mutter/RemoteDesktop";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // ONE connection for everything. Mutter ties a session's life to the D-Bus
    // connection that created it — a session made from a short-lived client is
    // gone the moment that client exits, which is what makes this impossible to
    // test with `busctl` one call at a time.
    let conn = zbus::Connection::session().await?;

    // RemoteDesktop first: the ScreenCast session is created already knowing
    // which one it belongs to. The binding is a creation property, not
    // something that can be attached afterwards.
    let rd = zbus::Proxy::new(&conn, RD_BUS, RD_PATH, RD_BUS).await?;
    let rd_session: OwnedObjectPath = rd.call("CreateSession", &()).await?;
    let rd_session_proxy =
        zbus::Proxy::new(&conn, RD_BUS, rd_session.clone(), format!("{RD_BUS}.Session")).await?;
    let rd_id: String = rd_session_proxy.get_property("SessionId").await?;
    println!("remote-desktop session: {rd_id}");

    let sc = zbus::Proxy::new(&conn, SC_BUS, SC_PATH, SC_BUS).await?;
    let mut session_props: HashMap<&str, Value> = HashMap::new();
    session_props.insert("remote-desktop-session-id", Value::from(rd_id));
    let session: OwnedObjectPath = sc.call("CreateSession", &(session_props,)).await?;
    let session_proxy =
        zbus::Proxy::new(&conn, SC_BUS, session.clone(), format!("{SC_BUS}.Session")).await?;

    let mut props: HashMap<&str, Value> = HashMap::new();
    // Cursor as metadata, never composited: compositing into a VIRTUAL monitor
    // makes Mutter tear the session down after 30-75 seconds. Measured before,
    // recorded in virtual_display.rs, repeated here so this spike does not
    // rediscover it as a mystery.
    props.insert("cursor-mode", Value::U32(2));
    let stream: OwnedObjectPath = session_proxy.call("RecordVirtual", &(props,)).await?;
    println!("stream: {stream}");

    // ONLY the RemoteDesktop session is started. Starting the ScreenCast one
    // directly fails with "Must be started from remote desktop session" once
    // the two are paired — the pairing makes the remote-desktop session the
    // owner, and starting it brings its stream up with it.
    rd_session_proxy.call::<_, _, ()>("Start", &()).await?;
    println!("session started — a new display should exist now");
    println!("walking the pointer across it…");

    // Absolute positions in the VIRTUAL monitor's own pixels. If the pairing
    // works, the cursor appears on the new display and not on the real one.
    for i in 0..70 {
        let x = 40.0 + f64::from(i) * 18.0;
        let y = 200.0 + f64::from(i % 10) * 12.0;
        if let Err(e) = rd_session_proxy
            .call::<_, _, ()>("NotifyPointerMotionAbsolute", &(stream.as_str(), x, y))
            .await
        {
            println!("FAIL: NotifyPointerMotionAbsolute: {e}");
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }

    println!("clicking once");
    let _ = rd_session_proxy
        .call::<_, _, ()>("NotifyPointerButton", &(0x110_i32, true))
        .await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let _ = rd_session_proxy
        .call::<_, _, ()>("NotifyPointerButton", &(0x110_i32, false))
        .await;

    tokio::time::sleep(Duration::from_secs(2)).await;
    let _ = session_proxy.call::<_, _, ()>("Stop", &()).await;
    println!("done — nothing is left configured on this machine");
    Ok(())
}
