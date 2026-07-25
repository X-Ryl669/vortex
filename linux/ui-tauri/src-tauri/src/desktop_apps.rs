//! Match a mirrored phone-notification's app to an installed LINUX desktop app,
//! and launch it — the "open the app on the laptop" leg of notification click.
//!
//! Dynamic by design: we index every installed `.desktop` file by its `Name=`
//! (and `StartupWMClass=`) and match the notification's app LABEL against it —
//! NO per-app hardcoding. So a phone "Telegram" (pkg `org.telegram.messenger`)
//! opens Linux `org.telegram.desktop` (Name=Telegram) purely by name; the same
//! path opens Slack, Discord, Signal, a native mail client, … whenever one is
//! installed. When nothing matches, the caller falls back to dismiss-only.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Lowercase + strip everything but a–z/0–9 so "Telegram Desktop", "telegram"
/// and "TelegramDesktop" (StartupWMClass) all collapse to the same key.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// The directories freedesktop looks in for `.desktop` files: XDG_DATA_HOME +
/// XDG_DATA_DIRS (each `/applications`), plus the flatpak + snap export dirs
/// (not always present in XDG_DATA_DIRS, so add them explicitly).
fn app_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();
    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{home}/.local/share"));
    dirs.push(PathBuf::from(format!("{data_home}/applications")));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    for d in data_dirs.split(':').filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(format!("{d}/applications")));
    }
    // flatpak + snap exports (commonly outside XDG_DATA_DIRS)
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    dirs.push(PathBuf::from(format!(
        "{data_home}/flatpak/exports/share/applications"
    )));
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));
    dirs
}

/// One entry per installed app: normalized name/wm-class → the `.desktop` file
/// path. Built once per session (installed apps rarely change under us; a click
/// re-scan would be wasteful). Later dirs don't overwrite earlier ones, so a
/// user-local override (`~/.local/share`) wins over a system copy.
fn index() -> &'static HashMap<String, PathBuf> {
    static INDEX: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut map: HashMap<String, PathBuf> = HashMap::new();
        for dir in app_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                // Only the [Desktop Entry] group's own Name/StartupWMClass, and
                // skip NoDisplay/Hidden launchers (background helpers, "Quit X").
                let mut in_main = false;
                let mut keys: Vec<String> = Vec::new();
                let mut hidden = false;
                for line in text.lines() {
                    let line = line.trim();
                    if line.starts_with('[') {
                        in_main = line == "[Desktop Entry]";
                        continue;
                    }
                    if !in_main {
                        continue;
                    }
                    if let Some(v) = line.strip_prefix("Name=") {
                        keys.push(norm(v));
                    } else if let Some(v) = line.strip_prefix("StartupWMClass=") {
                        keys.push(norm(v));
                    } else if line == "NoDisplay=true" || line == "Hidden=true" {
                        hidden = true;
                    }
                }
                if hidden {
                    continue;
                }
                for k in keys {
                    if !k.is_empty() {
                        map.entry(k).or_insert_with(|| path.clone());
                    }
                }
            }
        }
        map
    })
}

/// Find an installed desktop app whose name matches the notification's app
/// [label]. Exact normalized match first, then a contains-either-way pass so
/// "WhatsApp" ↔ "WhatsApp Web" style differences still resolve. Returns the
/// `.desktop` file path to launch, or None when nothing plausible is installed.
pub(crate) fn match_label(label: &str) -> Option<PathBuf> {
    let want = norm(label);
    if want.len() < 3 {
        return None; // too short to match safely (avoid "Go"→random app)
    }
    let idx = index();
    if let Some(p) = idx.get(&want) {
        return Some(p.clone());
    }
    idx.iter()
        .find(|(k, _)| k.contains(&want) || want.contains(k.as_str()))
        .map(|(_, p)| p.clone())
}

/// Launch a `.desktop` file. `gio launch` runs its Exec (handling field codes)
/// with no XDG-lookup dependency; if gio is missing, fall back to `gtk-launch`
/// by desktop-id. Fire-and-forget — a failure just means the app didn't open.
pub(crate) fn launch(desktop: &std::path::Path) {
    if tokio::process::Command::new("gio")
        .arg("launch")
        .arg(desktop)
        .spawn()
        .is_ok()
    {
        return;
    }
    if let Some(id) = desktop.file_stem().and_then(|s| s.to_str()) {
        let _ = tokio::process::Command::new("gtk-launch").arg(id).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::norm;

    #[test]
    fn norm_collapses_variants() {
        assert_eq!(norm("Telegram Desktop"), "telegramdesktop");
        assert_eq!(norm("TelegramDesktop"), "telegramdesktop");
        assert_eq!(norm("Signal"), "signal");
        assert_eq!(norm(""), "");
    }
}
