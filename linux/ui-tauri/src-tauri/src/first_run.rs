//! Per-user setup the app does for itself, so a PACKAGE install is complete.
//!
//! `install_linux.sh` writes the autostart entry and enables the GNOME
//! extension. A `.deb` or `.rpm` cannot do either: both are per-user choices
//! living under `$HOME`, and a system package that wrote there would be doing
//! it for whichever user happened to run `dnf`. So the app does them on
//! startup, idempotently — which also repairs an install whose files were
//! removed by hand.
//!
//! Nothing here is a first-run-only flag. Each step checks the world and acts
//! only if it needs to, so it is safe on every launch and self-healing.

use std::path::PathBuf;

const EXT_UUID: &str = "vortex-live@vortex";

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".local/share")))
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".config")))
}

/// Run the per-user setup. Cheap and quiet when there is nothing to do.
pub fn ensure() {
    ensure_autostart();
    ensure_gnome_extension();
}

/// The app lives in the tray and owns the BLE/LAN link, so it has to come up
/// with the session — without it, nothing works until the user launches it.
///
/// Written only when ABSENT: an entry the user deleted deliberately, or edited,
/// stays as they left it. Deleting it is how you turn autostart off.
fn ensure_autostart() {
    let Some(dir) = config_home().map(|c| c.join("autostart")) else { return };
    let path = dir.join("vortex-ui-tauri.desktop");
    if path.exists() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    // No `env …` prefix: the binary sets GDK_BACKEND and the WebKit renderer
    // flag itself now (see main.rs), so the entry stays valid even if the
    // binary moves or the packaging changes.
    //
    // The 8-second delay lets BlueZ, the Secret Service and the tray host
    // settle; starting into a session that has none of them yet costs a
    // retry cycle at every layer.
    let body = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Vortex\n\
         Comment=Phone companion — starts with the session\n\
         Exec={} --hidden\n\
         Icon=vortex-ui-tauri\n\
         Terminal=false\n\
         Categories=Utility;Network;\n\
         StartupNotify=false\n\
         StartupWMClass=vortex-ui-tauri\n\
         X-GNOME-Autostart-enabled=true\n\
         X-GNOME-Autostart-Delay=8\n",
        exe.display()
    );
    match std::fs::write(&path, body) {
        Ok(()) => tracing::info!("first-run: autostart entry written to {}", path.display()),
        Err(e) => tracing::warn!("first-run: could not write the autostart entry: {e}"),
    }
}

/// Enable the GNOME Shell extension that draws the live pill.
///
/// A package installs the extension system-wide, but ENABLING it is per-user
/// and lives in dconf, so the package cannot do it. Both the packaged location
/// and the one `install_linux.sh` uses are accepted.
fn ensure_gnome_extension() {
    if !on_gnome() {
        return;
    }
    if !extension_present() {
        return;
    }
    if enabled_list().is_some_and(|l| l.iter().any(|e| e == EXT_UUID)) {
        return; // already on, or already queued for the next login
    }
    // `gnome-extensions enable` is a D-Bus call into the RUNNING shell. Under
    // Wayland a freshly installed extension is invisible to it, so the call
    // fails with "does not exist" and — the part that matters — writes nothing
    // to dconf, leaving the extension installed-but-off forever. Writing the
    // list ourselves is what makes it come up enabled after the next login.
    if run("gnome-extensions", &["enable", EXT_UUID]) {
        tracing::info!("first-run: GNOME pill extension enabled");
        return;
    }
    let Some(mut list) = enabled_list() else { return };
    list.push(EXT_UUID.to_string());
    let value = format!(
        "[{}]",
        list.iter().map(|e| format!("'{e}'")).collect::<Vec<_>>().join(", ")
    );
    if run("gsettings", &["set", "org.gnome.shell", "enabled-extensions", &value]) {
        tracing::info!("first-run: GNOME pill extension queued — visible after the next login");
    } else {
        tracing::info!(
            "first-run: could not enable the pill extension; \
             `gnome-extensions enable {EXT_UUID}` does it by hand"
        );
    }
}

fn on_gnome() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_lowercase().contains("gnome"))
        .unwrap_or(false)
}

/// Installed for this user, or system-wide by a package.
fn extension_present() -> bool {
    let user = data_home().map(|d| d.join("gnome-shell/extensions").join(EXT_UUID));
    let system = PathBuf::from("/usr/share/gnome-shell/extensions").join(EXT_UUID);
    user.is_some_and(|p| p.join("metadata.json").exists()) || system.join("metadata.json").exists()
}

/// The current `enabled-extensions` list, parsed from its GVariant form.
fn enabled_list() -> Option<Vec<String>> {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.shell", "enabled-extensions"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let raw = raw.trim().trim_start_matches("@as ").trim();
    let inner = raw.strip_prefix('[')?.strip_suffix(']')?;
    Some(
        inner
            .split(',')
            .map(|e| e.trim().trim_matches('\'').trim_matches('"').to_string())
            .filter(|e| !e.is_empty())
            .collect(),
    )
}

fn run(cmd: &str, args: &[&str]) -> bool {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}
