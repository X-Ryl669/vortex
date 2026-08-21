//! Linux implementations of the platform seam.
//!
//! These delegate to the modules that already existed — the seam is a boundary,
//! not a rewrite, so behaviour on Linux is unchanged by construction.

use std::path::{Path, PathBuf};

use super::{BoxFuture, Notifier, SessionControl, UserPaths};

pub struct LinuxPaths;

impl UserPaths for LinuxPaths {
    /// The real XDG download directory (`~/Téléchargements` on a French
    /// desktop), never a hardcoded English `~/Downloads` — that mistake
    /// silently created a second folder beside the real one and filed every
    /// received file where the user never looks.
    fn downloads(&self) -> Option<PathBuf> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(xdg_download_dir(&home).unwrap_or_else(|| home.join("Downloads")))
    }

    fn config(&self) -> Option<PathBuf> {
        Some(config_home()?.join("vortex"))
    }

    fn cache(&self) -> Option<PathBuf> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".cache"));
        Some(base.join("vortex"))
    }
}

fn config_home() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config")),
    )
}

/// `XDG_DOWNLOAD_DIR` from the environment, else from the `user-dirs.dirs` file
/// `xdg-user-dir(1)` reads. Not required to exist — a configured-but-missing
/// folder is still the user's stated intent, and the caller creates it.
fn xdg_download_dir(home: &Path) -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("XDG_DOWNLOAD_DIR") {
        if let Some(p) = expand_home(&v.to_string_lossy(), home) {
            return Some(p);
        }
    }
    let text = std::fs::read_to_string(config_home()?.join("user-dirs.dirs")).ok()?;
    expand_home(&parse_user_dirs(&text, "XDG_DOWNLOAD_DIR")?, home)
}

/// Pull one key out of a `user-dirs.dirs` file: shell syntax, `# comment` lines
/// and `KEY="value"` assignments, last assignment winning as a shell would.
fn parse_user_dirs(text: &str, key: &str) -> Option<String> {
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(v);
        if !v.is_empty() {
            found = Some(v.to_string());
        }
    }
    found
}

/// Expand the `$HOME/…` (or `~/…`) prefix the spec mandates. Anything else must
/// already be absolute — a bare relative path is malformed, and guessing could
/// scatter files into the process's cwd.
fn expand_home(raw: &str, home: &Path) -> Option<PathBuf> {
    let raw = raw.trim();
    for prefix in ["$HOME", "${HOME}", "~"] {
        if let Some(rest) = raw.strip_prefix(prefix) {
            let rest = rest.trim_start_matches('/');
            return Some(if rest.is_empty() {
                home.to_path_buf()
            } else {
                home.join(rest)
            });
        }
    }
    let p = PathBuf::from(raw);
    p.is_absolute().then_some(p)
}

pub struct LinuxNotifier;

impl Notifier for LinuxNotifier {
    fn show(
        &self,
        summary: &str,
        body: &str,
        app_id: &str,
        actions: &[(String, String)],
        replaces: u32,
        urgent: bool,
    ) -> BoxFuture<Result<u32, String>> {
        let (summary, body, app_id) = (summary.to_string(), body.to_string(), app_id.to_string());
        let actions = actions.to_vec();
        Box::pin(async move {
            crate::core::notification_display::show_call_banner(
                &summary, &body, &app_id, &actions, replaces, urgent,
            )
            .await
        })
    }

    fn close(&self, id: u32) -> BoxFuture<Result<(), String>> {
        Box::pin(async move { crate::core::notification_display::close(id).await })
    }

    fn actions(&self, tx: tokio::sync::mpsc::UnboundedSender<(u32, String)>) {
        tokio::spawn(crate::core::notification_display::watch_actions(tx));
    }

    fn closures(&self, tx: tokio::sync::mpsc::UnboundedSender<(u32, u32)>) {
        tokio::spawn(crate::core::notification_display::watch_closed(tx));
    }
}

pub struct LinuxSession;

impl SessionControl for LinuxSession {
    fn lock(&self) -> BoxFuture<Result<(), String>> {
        Box::pin(crate::core::session_lock::lock())
    }

    fn unlock(&self) -> BoxFuture<Result<(), String>> {
        Box::pin(crate::core::session_lock::unlock())
    }

    fn is_locked(&self) -> BoxFuture<Option<bool>> {
        Box::pin(crate::core::session_lock::locked_hint())
    }

    /// logind can unlock, given the one-time polkit rule.
    fn can_unlock(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real French `user-dirs.dirs` — the case that made this code necessary.
    const FR: &str = r#"# This file is written by xdg-user-dirs-update
XDG_DESKTOP_DIR="$HOME/Bureau"
XDG_DOWNLOAD_DIR="$HOME/Téléchargements"
XDG_DOCUMENTS_DIR="$HOME/Documents"
"#;

    #[test]
    fn parses_localised_download_dir() {
        let raw = parse_user_dirs(FR, "XDG_DOWNLOAD_DIR").expect("download dir");
        assert_eq!(raw, "$HOME/Téléchargements");
        assert_eq!(
            expand_home(&raw, Path::new("/home/cyril")),
            Some(PathBuf::from("/home/cyril/Téléchargements"))
        );
    }

    #[test]
    fn ignores_comments_and_other_keys() {
        assert_eq!(parse_user_dirs(FR, "XDG_MUSIC_DIR"), None);
        let text = "#XDG_DOWNLOAD_DIR=\"$HOME/nope\"\nXDG_DOWNLOAD_DIR=\"$HOME/yes\"\n";
        assert_eq!(
            parse_user_dirs(text, "XDG_DOWNLOAD_DIR"),
            Some("$HOME/yes".to_string())
        );
    }

    #[test]
    fn last_assignment_wins_like_a_shell() {
        let text = "XDG_DOWNLOAD_DIR=\"$HOME/first\"\nXDG_DOWNLOAD_DIR=\"$HOME/second\"\n";
        assert_eq!(
            parse_user_dirs(text, "XDG_DOWNLOAD_DIR"),
            Some("$HOME/second".to_string())
        );
    }

    #[test]
    fn expands_home_forms_and_rejects_relative() {
        let home = Path::new("/home/cyril");
        for raw in ["$HOME/Dl", "${HOME}/Dl", "~/Dl"] {
            assert_eq!(expand_home(raw, home), Some(PathBuf::from("/home/cyril/Dl")));
        }
        assert_eq!(expand_home("$HOME/", home), Some(home.to_path_buf()));
        assert_eq!(expand_home("/data/dl", home), Some(PathBuf::from("/data/dl")));
        assert_eq!(expand_home("Downloads", home), None);
        assert_eq!(expand_home("", home), None);
    }

    #[test]
    fn handles_unquoted_and_single_quoted() {
        assert_eq!(
            parse_user_dirs("XDG_DOWNLOAD_DIR=$HOME/Dl\n", "XDG_DOWNLOAD_DIR"),
            Some("$HOME/Dl".to_string())
        );
        assert_eq!(
            parse_user_dirs("XDG_DOWNLOAD_DIR='$HOME/Dl'\n", "XDG_DOWNLOAD_DIR"),
            Some("$HOME/Dl".to_string())
        );
    }
}

