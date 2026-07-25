//! Smart audio-follow on/off persistence.
//!
//! Unlike locale/theme (per-device, frontend-localStorage), the smart-switch
//! toggle is a SHARED system setting synced cross-device last-writer-wins.
//! The daemon owns the persisted `(enabled, changed_at)` so it can build the
//! outgoing AppState and resolve LWW even with no UI focused, and re-seed the
//! runtime flag across restarts.
//!
//! Wire format: a single JSON object in $XDG_CONFIG_HOME/vortex/
//! smart_switch.json:
//!
//!   { "enabled": true, "changed_at": 1700000000 }

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmartSwitch {
    pub enabled: bool,
    /// Unix-seconds timestamp of the last explicit local toggle. The LWW
    /// key — the side with the greater value wins.
    pub changed_at: u64,
}

impl Default for SmartSwitch {
    fn default() -> Self {
        // Enabled out of the box; changed_at 0 means "no opinion yet", so
        // any explicit peer toggle (changed_at > 0) wins.
        SmartSwitch { enabled: true, changed_at: 0 }
    }
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("vortex").join("smart_switch.json"))
}

/// Load the saved setting, or the default (enabled, ts=0) when absent.
pub fn load() -> SmartSwitch {
    config_path()
        .and_then(|p| std::fs::read(&p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(value: &SmartSwitch) -> std::io::Result<()> {
    let path = config_path()
        .ok_or_else(|| std::io::Error::other("no config dir"))?;
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(std::io::Error::other)?;
    crate::core::fs_private::write_private(&path, &bytes)
}
