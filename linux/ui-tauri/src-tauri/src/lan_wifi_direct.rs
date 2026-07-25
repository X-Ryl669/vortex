//! Wi-Fi Direct fast transfer (instant-share style) — split out of `lan.rs`. The phone
//! makes a 5 GHz P2P group; the laptop joins it (nmcli), pulls pending files
//! over the ~20 MB/s direct link, then restores its normal Wi-Fi (single adapter
//! ⇒ briefly offline). `lan.rs`'s heartbeat reads `wd_active()` to target the GO
//! IP, and calls `restore_wifi` once the pull drains.

use std::time::Duration;

use tauri::{AppHandle, Emitter};

// --------------------------------------------------------------------------
// Wi-Fi Direct fast transfer (instant-share style): the phone makes a 5 GHz P2P group;
// the laptop joins it, pulls pending files over the direct link (~20 MB/s vs the
// router path), then restores its normal Wi-Fi. Single adapter ⇒ briefly offline.
// --------------------------------------------------------------------------

/// Android always puts the P2P group owner here; `LanServer` binds all ifaces.
pub(crate) const WIFI_DIRECT_GO_IP: [u8; 4] = [192, 168, 49, 1];

pub(crate) struct WdState {
    /// The Wi-Fi connection to restore after the transfer (None = unknown).
    saved_wifi: Option<String>,
}

pub(crate) static WIFI_DIRECT: std::sync::Mutex<Option<WdState>> = std::sync::Mutex::new(None);

pub(crate) fn wd_active() -> bool {
    WIFI_DIRECT.lock().map(|g| g.is_some()).unwrap_or(false)
}

async fn nmcli(args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new("nmcli")
        .args(args)
        .output()
        .await
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

/// The active Wi-Fi connection name (to restore later).
async fn current_wifi() -> Option<String> {
    let s = nmcli(&["-t", "-f", "TYPE,CONNECTION", "device", "status"]).await?;
    s.lines()
        .find_map(|l| l.strip_prefix("wifi:").map(str::to_string))
        .filter(|c| !c.is_empty() && c != "--")
}

/// nmcli that also returns stderr (for diagnosing a failed join).
async fn nmcli_err(args: &[&str]) -> Result<(), String> {
    match tokio::process::Command::new("nmcli").args(args).output().await {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Join the phone's P2P group. A fresh 5 GHz GO isn't in the cached scan, so
/// rescan before EACH attempt. A STALE NM profile for this SSID (left by a prior
/// join) loses its security config → "key-mgmt property is missing"; delete it
/// first so `dev wifi connect` recreates it cleanly from the live scan.
async fn join_go(ssid: &str, pass: &str) -> bool {
    let _ = nmcli(&["con", "delete", ssid]).await; // ignore "unknown connection"
    for attempt in 1..=5 {
        let _ = nmcli(&["dev", "wifi", "rescan"]).await;
        tokio::time::sleep(Duration::from_secs(4)).await;
        match nmcli_err(&["dev", "wifi", "connect", ssid, "password", pass]).await {
            Ok(()) => return true,
            Err(e) => {
                tracing::warn!(attempt, "Wi-Fi Direct join attempt failed: {e}");
                // Drop the half-made profile so the next attempt starts clean.
                let _ = nmcli(&["con", "delete", ssid]).await;
            }
        }
    }
    false
}

/// Restore the saved Wi-Fi and clear the Wi-Fi-Direct state. Idempotent.
pub(crate) async fn restore_wifi(app: &AppHandle) {
    let saved = WIFI_DIRECT
        .lock()
        .ok()
        .and_then(|mut g| g.take())
        .and_then(|s| s.saved_wifi);
    if let Some(name) = saved {
        let _ = nmcli(&["con", "up", &name]).await;
        tracing::info!(name = %name, "Wi-Fi Direct: Wi-Fi restored");
    } else {
        // We never captured the pre-join Wi-Fi (current_wifi() failed at join
        // time), so there's no specific connection to bring back. With a single
        // adapter the laptop is otherwise stranded on the GO group — force the
        // Wi-Fi radio to re-associate with the strongest known AP so it doesn't
        // stay offline. Best-effort: a radio off/on triggers NM auto-connect.
        tracing::warn!("Wi-Fi Direct: no saved Wi-Fi to restore; cycling radio to auto-reconnect");
        let _ = nmcli(&["radio", "wifi", "off"]).await;
        let _ = nmcli(&["radio", "wifi", "on"]).await;
    }
    let _ = app.emit("vortex:wifi-direct", false);
}

/// Hook target (set in the worker): the phone offered a P2P group. If files are
/// pending, switch onto it so the heartbeat pulls them over the fast link.
pub(crate) fn on_wifi_direct_offer(app: AppHandle, ssid: String, pass: String) {
    let pending = crate::PENDING_FILE_OFFERS
        .get()
        .and_then(|m| m.lock().ok().map(|g| !g.is_empty()))
        .unwrap_or(false);
    if !pending || wd_active() {
        return;
    }
    tokio::spawn(async move {
        let saved = current_wifi().await;
        tracing::info!(?saved, %ssid, "Wi-Fi Direct: joining group for fast pull");
        if !join_go(&ssid, &pass).await {
            tracing::warn!("Wi-Fi Direct: join failed; staying on router path");
            return;
        }
        if let Ok(mut g) = WIFI_DIRECT.lock() {
            *g = Some(WdState { saved_wifi: saved });
        }
        let _ = app.emit("vortex:wifi-direct", true);
        if let Some(n) = crate::SYNC_NUDGE.get() {
            n.notify_one(); // pull now over the GO
        }
        // Watchdog: never strand the laptop on the GO (failed pull / lost link).
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if wd_active() {
                tracing::warn!("Wi-Fi Direct: watchdog timeout → force-restore Wi-Fi");
                restore_wifi(&app2).await;
            }
        });
    });
}
