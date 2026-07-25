//! Decoupling hook for the Wi-Fi Direct "ready" signal (same pattern as
//! `file_progress`): the BLE listener (this crate) parses a `WIFI_DIRECT_OFFER`
//! frame and calls [`report`]; the Tauri UI (binary crate) installs a hook that
//! switches the laptop onto the phone's P2P group and pulls pending files over
//! the direct link. No-op until a hook is set.

use std::sync::OnceLock;

type Hook = Box<dyn Fn(String, String) + Send + Sync>;

static HOOK: OnceLock<Hook> = OnceLock::new();

/// Install the sink (the UI's Wi-Fi-Direct fast-transfer trigger). First wins.
pub fn set_hook(hook: Hook) {
    let _ = HOOK.set(hook);
}

/// The phone announced a Wi-Fi Direct group: `ssid` + `pass` to join it.
pub fn report(ssid: String, pass: String) {
    if let Some(h) = HOOK.get() {
        h(ssid, pass);
    }
}
