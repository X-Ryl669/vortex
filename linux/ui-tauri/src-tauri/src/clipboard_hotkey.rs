//! GNOME custom-shortcut registration for the clipboard popup (gsettings) —
//! split out of `clipboard.rs`. Self-contained: only shells out to `gsettings`
//! to bind/unbind the Super+V shortcut that launches `vortex-ui-tauri
//! --clipboard`. No coupling to the clipboard store/sync internals.

const KEYS_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const CUSTOM_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/vortex-clipboard/";

fn gsettings(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("gsettings")
        .args(args)
        .output()
        .map_err(|e| format!("gsettings: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Register (or update) the GNOME custom shortcut that opens the popup.
/// `binding` uses gsettings syntax, e.g. `<Super>v`. When the binding is
/// Super+V we also remove it from GNOME's notification-list shortcut
/// (which holds it by default) — Super+M keeps doing that job.
pub fn set_clipboard_hotkey(binding: String) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("current_exe: {e}"))?
        .to_string_lossy()
        .into_owned();

    // Free Super+V from GNOME's notification list if we're claiming it.
    if binding.to_lowercase() == "<super>v" {
        if let Ok(cur) = gsettings(&["get", "org.gnome.shell.keybindings", "toggle-message-tray"]) {
            if cur.to_lowercase().contains("<super>v") {
                // Keep every other binding in the list (usually <Super>m).
                let kept: Vec<String> = cur
                    .trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(',')
                    .map(|s| s.trim().trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty() && s.to_lowercase() != "<super>v")
                    .map(|s| format!("'{s}'"))
                    .collect();
                let _ = gsettings(&[
                    "set",
                    "org.gnome.shell.keybindings",
                    "toggle-message-tray",
                    &format!("[{}]", kept.join(", ")),
                ]);
            }
        }
    }

    // Ensure our path is in the custom-keybindings list.
    let list = gsettings(&["get", KEYS_SCHEMA, "custom-keybindings"]).unwrap_or_default();
    if !list.contains(CUSTOM_PATH) {
        let entries: Vec<String> = list
            .trim()
            .trim_start_matches("@as")
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|s| s.trim().trim_matches('\'').to_string())
            .filter(|s| !s.is_empty())
            .chain(std::iter::once(CUSTOM_PATH.to_string()))
            .map(|s| format!("'{s}'"))
            .collect();
        gsettings(&[
            "set",
            KEYS_SCHEMA,
            "custom-keybindings",
            &format!("[{}]", entries.join(", ")),
        ])?;
    }

    let schema_path = format!("{KEYS_SCHEMA}.custom-keybinding:{CUSTOM_PATH}");
    gsettings(&["set", &schema_path, "name", "Vortex Clipboard"])?;
    gsettings(&["set", &schema_path, "command", &format!("{exe} --clipboard")])?;
    gsettings(&["set", &schema_path, "binding", &binding])?;
    Ok(())
}
