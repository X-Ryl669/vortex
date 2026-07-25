//! D-Bus publisher for live activities, consumed by the OPTIONAL Vortex GNOME
//! Shell extension (which draws a live-activity pill + card in the panel). The
//! system-tray fallback in the UI still works when the extension isn't present
//! — this is purely additive.
//!
//! Exposes `org.vortex.LiveActivities` at `/org/vortex/LiveActivities` with an
//! `Activities` property (a JSON array of the currently-active live
//! activities). The extension reads it and re-reads on PropertiesChanged.

use zbus::interface;

pub struct LiveActivities {
    json: String,
    /// Forwards a call-card action (the extension clicked Mute/Speaker/End on
    /// the in-call pill) to the call module, which sends it to the phone.
    call_action_tx: tokio::sync::mpsc::UnboundedSender<String>,
}

#[interface(name = "org.vortex.LiveActivities1")]
impl LiveActivities {
    /// JSON array: [{app, app_id, title, text, sub, progress, icon}].
    #[zbus(property)]
    async fn activities(&self) -> String {
        self.json.clone()
    }

    /// The GNOME extension calls this when the user clicks an action on the
    /// in-call pill's card. `action` is a verb (e.g. "mute", "end", "speaker_on").
    async fn call_action(&self, action: String) {
        tracing::info!(action = %action, "call-card action from extension");
        let _ = self.call_action_tx.send(action);
    }
}

/// Start the publisher. Returns a sender; push the JSON array of active live
/// activities whenever it changes and it's published to the extension.
/// `call_action_tx` receives in-call-pill action verbs the extension invokes.
pub async fn start(
    call_action_tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<tokio::sync::mpsc::UnboundedSender<String>, String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let conn = zbus::connection::Builder::session()
        .map_err(|e| format!("session bus: {e}"))?
        .name("org.vortex.LiveActivities")
        .map_err(|e| format!("request name: {e}"))?
        .serve_at(
            "/org/vortex/LiveActivities",
            LiveActivities { json: "[]".to_string(), call_action_tx },
        )
        .map_err(|e| format!("serve_at: {e}"))?
        .build()
        .await
        .map_err(|e| format!("build: {e}"))?;
    tracing::info!("live-activity D-Bus publisher up (org.vortex.LiveActivities)");
    tokio::spawn(async move {
        // Hold the connection for the daemon's lifetime.
        let conn = conn;
        let iface_ref = match conn
            .object_server()
            .interface::<_, LiveActivities>("/org/vortex/LiveActivities")
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("live-activity dbus: interface lookup: {e}");
                return;
            }
        };
        while let Some(json) = rx.recv().await {
            let mut iface = iface_ref.get_mut().await;
            iface.json = json;
            let _ = iface
                .activities_changed(iface_ref.signal_emitter())
                .await;
        }
    });
    Ok(tx)
}
