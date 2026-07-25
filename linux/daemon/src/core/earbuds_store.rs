//! User-selected earbuds persistence.
//!
//! Once the user picks an audio device through the in-app modal, we
//! remember it forever (per install) — the card stays on the home
//! screen whether the buds are currently connected or in the case.
//! Connection state + battery are looked up at render time via BlueZ;
//! this file only stores the *identity* the user picked.
//!
//! Wire format: a single JSON object in $XDG_CONFIG_HOME/vortex/
//! earbuds.json, e.g.
//!
//!   { "address": "AC:47:1B:25:71:C2",
//!     "name":    "HUAWEI FreeBuds SE 3" }
//!
//! We don't put this in Secret Service because there's nothing
//! sensitive about it — it's the same info `bluetoothctl devices`
//! prints. Keeping it as plain JSON makes it trivial to inspect or
//! reset by hand.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedEarbuds {
    /// Bluetooth address in standard `AA:BB:CC:DD:EE:FF` form.
    pub address: String,
    /// Friendly name at the time the user saved it. We refresh this
    /// on every successful BlueZ lookup, but keep the saved value
    /// as a fallback when the device is out of range.
    pub name: String,
}

fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("vortex"))
}

fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("earbuds.json"))
}

/// One-shot marker: set the first time we run the first-launch
/// "adopt the already-connected earbuds" probe. We gate the probe on
/// this (not just on `load().is_none()`) so a user who later taps
/// "Remove from Vortex" doesn't have their buds silently re-adopted on
/// the next launch — the auto-detect is a one-time, fresh-install nicety.
fn autodetect_marker_path() -> Option<PathBuf> {
    Some(config_dir()?.join("earbuds_autodetect.done"))
}

pub fn autodetect_done() -> bool {
    autodetect_marker_path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

pub fn mark_autodetect_done() -> std::io::Result<()> {
    let path = autodetect_marker_path()
        .ok_or_else(|| std::io::Error::other("no config dir"))?;
    crate::core::fs_private::write_private(&path, b"1")
}

pub fn load() -> Option<SavedEarbuds> {
    let path = config_path()?;
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save(value: &SavedEarbuds) -> std::io::Result<()> {
    let path = config_path()
        .ok_or_else(|| std::io::Error::other("no config dir"))?;
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(std::io::Error::other)?;
    crate::core::fs_private::write_private(&path, &bytes)
}

pub fn clear() -> std::io::Result<()> {
    if let Some(path) = config_path() {
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    } else {
        Ok(())
    }
}
