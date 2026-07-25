//! Show a mirrored phone notification on the laptop desktop via the
//! standard `org.freedesktop.Notifications` D-Bus service (the same bus
//! `notify-send` uses). Best-effort — a missing notification daemon just
//! logs a warning; it never gates anything.

use zbus::{Connection, Proxy};

use crate::core::notif_mirror::NotificationMirror;

/// Quote + escape a string as a GVariant string literal for a `gdbus call`
/// argument, so arbitrary notification text (quotes, brackets, backslashes)
/// is passed verbatim and never misparsed.
fn gvariant_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Pop a desktop notification for a mirrored phone notification. The phone
/// app label becomes the summary prefix so the user sees which app it's
/// from. Content is taken as-is (already length-capped on the phone).
/// Returns the assigned notification id (for dismissal sync — we map it to
/// the phone's notification key).
pub async fn show(notif: &NotificationMirror, replaces_id: u32) -> Result<u32, String> {
    // `replaces_id` (0 = new) collapses repeated notifications from the same
    // chat into one that updates in place — standard messenger-style grouping.
    // The app name goes in the header (app_name), so the summary is just the
    // title (chat / sender), falling back to the app label when there's no
    // title. body = the notification text.
    let summary = if notif.title.is_empty() {
        notif.app.clone()
    } else {
        notif.title.clone()
    };

    // Action buttons: the freedesktop "actions" array is [key, label, key,
    // label, …]. We key each as "act:<index>" so ActionInvoked maps straight
    // back to the phone action index. The well-known "default" key makes the
    // notification BODY clickable (the server fires ActionInvoked("default")
    // on click) — the UI uses it to focus the app and jump to the chat.
    let mut action_parts: Vec<String> = vec![
        gvariant_string("default"),
        gvariant_string("Open"),
    ];
    for (i, label) in notif.actions.iter().enumerate() {
        action_parts.push(gvariant_string(&format!("act:{i}")));
        action_parts.push(gvariant_string(label));
    }
    let actions_arg = format!("[{}]", action_parts.join(", "));
    // Actionable notifications stay until dismissed (0); plain ones get a
    // short banner (GNOME drops a banner's buttons when it expires).
    let expire_timeout: i32 = if notif.actions.is_empty() { 8000 } else { 0 };

    // app_icon: the phone's real app logo if we've cached it (sent once over
    // BLE as ICON chunks), else the bundled generic bell. ALWAYS an absolute
    // path under ~/.cache/vortex/ so the laptop→phone capturer can recognise
    // (and skip) our own notifications by the icon path.
    let app_icon = crate::core::icon_cache::icon_path(&notif.app_id)
        .filter(|p| p.exists())
        .or_else(crate::core::icon_cache::ensure_generic)
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| "phone-symbolic".to_string());
    // Header app name = the real phone app (e.g. "Telegram"), not "Vortex".
    let app_name = if notif.app.is_empty() {
        "Phone".to_string()
    } else {
        notif.app.clone()
    };

    // CRITICAL: post via a short-lived `gdbus` child, NOT this daemon's own
    // D-Bus connection. GNOME associates a notification with its SENDER
    // process; because this daemon owns a (Tauri) window, gnome-shell
    // instantly auto-dismisses its notifications (NotificationClosed
    // reason=2 within ~ms, as if the user had already seen them) so they
    // never actually appear. A windowless sender (gdbus) has no window to
    // associate, so the notification stays up. Action/close callbacks still
    // arrive via our global sender-less signal watchers — nothing is lost by
    // not owning the connection here.
    let output = tokio::process::Command::new("gdbus")
        .arg("call")
        .arg("--session")
        .arg("--dest")
        .arg("org.freedesktop.Notifications")
        .arg("--object-path")
        .arg("/org/freedesktop/Notifications")
        .arg("--method")
        .arg("org.freedesktop.Notifications.Notify")
        .arg(gvariant_string(&app_name)) // app_name = real phone app
        .arg(replaces_id.to_string()) // replaces_id (u32): 0 = new, else update
        .arg(gvariant_string(&app_icon)) // app_icon (cached logo or generic)
        .arg(gvariant_string(&summary))
        // GNOME collapses newlines in a notification body to spaces (even
        // notify-send can't line-break), so stacked chat messages would run
        // together. Use a middle-dot separator so they stay distinguishable
        // on the one line GNOME gives us. (A future GNOME Shell extension
        // could render the raw newlines as real lines.)
        .arg(gvariant_string(&notif.text.replace('\n', "  ·  ")))
        .arg(&actions_arg) // as
        .arg("@a{sv} {}") // hints
        .arg(expire_timeout.to_string()) // i32
        .output()
        .await
        .map_err(|e| format!("gdbus spawn: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "gdbus Notify failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // gdbus prints "(uint32 78,)" — strip the type keyword first (it ends in
    // digits "32" which would otherwise be misread as the id), then take the
    // remaining integer.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let id: u32 = stdout
        .replace("uint32", " ")
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .map_err(|_| format!("gdbus Notify: unparseable id {stdout:?}"))?;
    Ok(id)
}

/// Close a desktop notification we previously showed (the phone dismissed
/// its original, so drop our mirrored copy). Emits a NotificationClosed
/// signal with reason=3 (closed-by-call), which the watcher ignores.
pub async fn close(id: u32) -> Result<(), String> {
    let conn = Connection::session()
        .await
        .map_err(|e| format!("session bus: {e}"))?;
    let proxy = Proxy::new(
        &conn,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .await
    .map_err(|e| format!("notifications proxy: {e}"))?;
    proxy
        .call::<_, _, ()>("CloseNotification", &(id,))
        .await
        .map_err(|e| format!("CloseNotification: {e}"))?;
    Ok(())
}

/// Pop (or update) the continuity-style incoming-call banner: a
/// persistent, critical-urgency desktop notification with caller info and
/// action buttons (Accept / Decline / …). `actions` is `(key, label)` pairs —
/// the call module keys them `call:<verb>` so its own ActionInvoked watcher
/// can route the click straight to a `CallControl` (disjoint from the
/// notification-mirror's `act:<n>` keys, so the two watchers never collide).
/// `replaces_id` (0 = new) updates the same banner in place across phases.
/// Posted via a windowless `gdbus` child for the same reason as [`show`].
pub async fn show_call_banner(
    title: &str,
    body: &str,
    app_id: &str,
    actions: &[(String, String)],
    replaces_id: u32,
    critical: bool,
) -> Result<u32, String> {
    // The phone's real dialer-app logo if we've cached it, else a themed call
    // glyph. Same cache the notification mirror fills over BLE.
    let icon = crate::core::icon_cache::icon_path(app_id)
        .filter(|p| p.exists())
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| "call-start-symbolic".to_string());
    let mut action_parts: Vec<String> = Vec::new();
    for (key, label) in actions {
        action_parts.push(gvariant_string(key));
        action_parts.push(gvariant_string(label));
    }
    let actions_arg = format!("[{}]", action_parts.join(", "));

    // urgency=critical (byte 2) → GNOME keeps the banner on screen until acted
    // on. When the user dismisses it (the "silence" gesture) we re-show at
    // urgency=normal (byte 1): it tucks quietly into the notification list (no
    // aggressive re-pop) but stays there with its Accept/Decline actions.
    // category 'call.incoming' lets shells style it.
    let hints = if critical {
        "{'urgency': <byte 2>, 'category': <'call.incoming'>}"
    } else {
        "{'urgency': <byte 1>, 'category': <'call.incoming'>}"
    };

    let output = tokio::process::Command::new("gdbus")
        .arg("call")
        .arg("--session")
        .arg("--dest")
        .arg("org.freedesktop.Notifications")
        .arg("--object-path")
        .arg("/org/freedesktop/Notifications")
        .arg("--method")
        .arg("org.freedesktop.Notifications.Notify")
        .arg(gvariant_string("Phone")) // app_name
        .arg(replaces_id.to_string()) // replaces_id (0 = new)
        .arg(gvariant_string(&icon)) // app_icon (real dialer logo or call glyph)
        .arg(gvariant_string(title)) // summary = caller
        .arg(gvariant_string(body)) // body = "Incoming call" / number
        .arg(&actions_arg) // as
        .arg(hints) // a{sv}
        .arg("0") // expire_timeout: never (critical + resident)
        .output()
        .await
        .map_err(|e| format!("gdbus spawn: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "gdbus call-banner failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let id: u32 = stdout
        .replace("uint32", " ")
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .map_err(|_| format!("gdbus call-banner: unparseable id {stdout:?}"))?;
    Ok(id)
}

/// Build a sender-less signal match rule for one of the Notifications
/// signals. CRUCIAL on GNOME: the `org.freedesktop.Notifications` well-known
/// name is owned by a relay (e.g. `:1.34`) that forwards to gnome-shell,
/// but the ActionInvoked / NotificationClosed signals are emitted by
/// gnome-shell under a DIFFERENT unique name (e.g. `:1.26`). A Proxy-based
/// subscription pins `sender=org.freedesktop.Notifications` → resolves to
/// the relay's name → filters OUT gnome-shell's signals, so we never see
/// them. Matching on interface+member+path only (no sender) catches the
/// real emitter.
fn signal_rule(member: &'static str) -> zbus::Result<zbus::MatchRule<'static>> {
    Ok(zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.Notifications")?
        .member(member)?
        .path("/org/freedesktop/Notifications")?
        .build())
}

/// Watch the `NotificationClosed(id, reason)` signal and forward each event
/// to `tx`. reason: 1=expired, 2=dismissed-by-user, 3=closed-by-CloseNotification,
/// 4=undefined. The caller uses reason==2 to sync a user dismissal back to
/// the phone (and ignores 3, which is our own [`close`]).
pub async fn watch_closed(tx: tokio::sync::mpsc::UnboundedSender<(u32, u32)>) {
    use futures::StreamExt;
    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("notif-closed watch: session bus: {e}");
            return;
        }
    };
    let rule = match signal_rule("NotificationClosed") {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("notif-closed watch: rule: {e}");
            return;
        }
    };
    let mut stream = match zbus::MessageStream::for_match_rule(rule, &conn, None).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("notif-closed watch: subscribe: {e}");
            return;
        }
    };
    tracing::info!("notif-closed watch: subscribed (sender-less rule)");
    while let Some(Ok(msg)) = stream.next().await {
        if let Ok((id, reason)) = msg.body().deserialize::<(u32, u32)>() {
            tracing::info!(id, reason, "notif-closed: NotificationClosed signal");
            let _ = tx.send((id, reason));
        }
    }
}

/// Watch the `ActionInvoked(id, action_key)` signal — the user clicked an
/// action button on a mirrored notification. Forwards (id, action_key) to
/// `tx`; the caller maps the id back to the phone key and the "act:<n>"
/// key to the action index, then asks the phone to fire it.
pub async fn watch_actions(tx: tokio::sync::mpsc::UnboundedSender<(u32, String)>) {
    use futures::StreamExt;
    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("notif-action watch: session bus: {e}");
            return;
        }
    };
    let rule = match signal_rule("ActionInvoked") {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("notif-action watch: rule: {e}");
            return;
        }
    };
    let mut stream = match zbus::MessageStream::for_match_rule(rule, &conn, None).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("notif-action watch: subscribe: {e}");
            return;
        }
    };
    tracing::info!("notif-action watch: subscribed (sender-less rule)");
    while let Some(Ok(msg)) = stream.next().await {
        if let Ok((id, key)) = msg.body().deserialize::<(u32, String)>() {
            tracing::info!(id, action = %key, "notif-action: ActionInvoked signal");
            let _ = tx.send((id, key));
        }
    }
}
