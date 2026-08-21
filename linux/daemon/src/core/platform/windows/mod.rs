//! Windows implementations of the platform seam.
//!
//! Compiled only on Windows, so nothing here can break the Linux build. Each
//! stub names the concrete API it will call, so the remaining work is visible
//! as a checklist rather than as "port it".
//!
//! Verify with
//! `cargo check -p vortex-l3-daemon --lib --target x86_64-pc-windows-gnu`
//! (needs `rustup target add x86_64-pc-windows-gnu` — Arch's packaged rustc
//! ships no Windows std). The `-msvc` target cannot be cross-checked from
//! Linux: a C dependency's build script wants `lib.exe` and fails before
//! reaching our code. Running any of this needs a real Windows machine or VM —
//! BLE and toast activation cannot be exercised from Linux at all.

use std::path::PathBuf;

use super::{BoxFuture, Notifier, SessionControl, UserPaths};

pub mod ble;

pub struct WindowsPaths;

/// Resolve a Windows known folder to a path.
///
/// This — not an environment variable — is the supported way to ask. Every one
/// of these folders can be REDIRECTED: OneDrive relocates Downloads and
/// Documents by default on a consumer machine, and a domain profile can move
/// AppData. `%USERPROFILE%\Downloads` is merely the common case, and getting it
/// wrong means writing received files into a folder the user never opens —
/// exactly the bug the Linux side already had with a hardcoded `~/Downloads`
/// on a French desktop.
///
/// `KF_FLAG_DONT_VERIFY` because a configured-but-missing folder is still the
/// user's stated intent: the receive path creates the directory anyway, and
/// verifying here would fail the lookup and send us to a fallback instead. Same
/// reasoning as the XDG side.
#[cfg(target_os = "windows")]
fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DONT_VERIFY};

    // SAFETY: `id` is one of the FOLDERID_* constants, and the out-pointer is a
    // COM allocation we own. It is freed on BOTH paths below — including the
    // UTF-16 conversion failure — before anything is returned.
    let raw = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY, None) }.ok()?;
    let text = unsafe { raw.to_string() };
    unsafe { CoTaskMemFree(Some(raw.0 as *const std::ffi::c_void)) };
    Some(PathBuf::from(text.ok()?))
}

impl UserPaths for WindowsPaths {
    /// The user's real Downloads folder, wherever it has been moved to.
    fn downloads(&self) -> Option<PathBuf> {
        known_folder(&windows::Win32::UI::Shell::FOLDERID_Downloads)
    }

    /// `%APPDATA%\Vortex` — roaming, so settings follow a domain profile.
    fn config(&self) -> Option<PathBuf> {
        Some(known_folder(&windows::Win32::UI::Shell::FOLDERID_RoamingAppData)?.join("Vortex"))
    }

    /// `%LOCALAPPDATA%\Vortex\Cache` — local, never roamed: the icon cache is
    /// machine-specific and would only bloat a roaming profile.
    fn cache(&self) -> Option<PathBuf> {
        Some(
            known_folder(&windows::Win32::UI::Shell::FOLDERID_LocalAppData)?
                .join("Vortex")
                .join("Cache"),
        )
    }
}

pub struct WindowsNotifier;

impl Notifier for WindowsNotifier {
    /// TODO: WinRT `ToastNotificationManager` with an `AppUserModelID`.
    ///
    /// The AUMID is the whole problem: an unpackaged exe has none until it
    /// registers a Start-menu shortcut carrying one, and without it Windows
    /// silently refuses to show the toast. Actions then need a registered COM
    /// activator (`INotificationActivationCallback`) — see [`Self::actions`].
    fn show(
        &self,
        _summary: &str,
        _body: &str,
        _app_id: &str,
        _actions: &[(String, String)],
        _replaces: u32,
        _urgent: bool,
    ) -> BoxFuture<Result<u32, String>> {
        Box::pin(async { Err("windows notifier: not implemented".to_string()) })
    }

    /// TODO: `ToastNotificationHistory::Remove` by tag. Windows keys toasts by
    /// string tag, not the u32 the freedesktop API returns, so the
    /// implementation keeps an id→tag map behind this signature.
    fn close(&self, _id: u32) -> BoxFuture<Result<(), String>> {
        Box::pin(async { Err("windows notifier: not implemented".to_string()) })
    }

    /// TODO: activation callback → `tx`.
    ///
    /// This is the piece with no Linux analogue. A toast button carries
    /// arguments; clicking it activates the app through COM, and the handler
    /// must translate those arguments back into the same `fc:` / `call:` /
    /// `act:` keys the existing consumers already filter on — so the routing
    /// above this trait needs no Windows-specific branch.
    fn actions(&self, _tx: tokio::sync::mpsc::UnboundedSender<(u32, String)>) {}

    /// TODO: `ToastNotification::Dismissed` / `Failed` events. Windows reports
    /// dismissal per-notification rather than as a bus signal, so this
    /// subscribes as toasts are created and fans them into the one channel.
    fn closures(&self, _tx: tokio::sync::mpsc::UnboundedSender<(u32, u32)>) {}
}

pub struct WindowsSession;

impl SessionControl for WindowsSession {
    /// TODO: `LockWorkStation()` from user32.
    fn lock(&self) -> BoxFuture<Result<(), String>> {
        Box::pin(async { Err("windows session lock: not implemented".to_string()) })
    }

    /// Windows has no programmatic unlock, by design — credentials must be
    /// presented to the LogonUI. Proximity auto-unlock is therefore Linux-only;
    /// [`SessionControl::can_unlock`] reports that so the UI can hide the
    /// setting instead of offering something that always fails.
    fn unlock(&self) -> BoxFuture<Result<(), String>> {
        Box::pin(async { Err("windows cannot unlock a session programmatically".to_string()) })
    }

    /// TODO: `WTSRegisterSessionNotification` + `WTS_SESSION_LOCK`/`_UNLOCK`,
    /// cached — there is no "is it locked right now" query on Windows, only
    /// the transition events, so state has to be tracked from process start.
    fn is_locked(&self) -> BoxFuture<Option<bool>> {
        Box::pin(async { None })
    }

    fn can_unlock(&self) -> bool {
        false
    }
}
