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

use super::{BoxFuture, SessionControl, UserPaths};

pub mod ble;
pub mod notify;

/// Join the multithreaded apartment on this thread, once.
///
/// WinRT activation fails with `CO_E_NOTINITIALIZED` on a thread that has not
/// initialized an apartment, and the daemon's threads are plain tokio workers
/// that never do. This compiles fine without it and then fails on the very
/// first call — the sort of thing only a real Windows run surfaces.
///
/// `S_FALSE` (already initialized) and `RPC_E_CHANGED_MODE` (this thread is
/// already in an STA — the UI thread, say) are both fine: in either case the
/// thread has an apartment, which is all we need.
fn ensure_winrt() {
    use std::cell::Cell;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
    thread_local! {
        static JOINED: Cell<bool> = const { Cell::new(false) };
    }
    JOINED.with(|j| {
        if j.get() {
            return;
        }
        // SAFETY: no arguments to get wrong; the failure modes above are the
        // documented benign ones and everything else means WinRT is unusable
        // here, which the next call will report with real context.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        j.set(true);
    });
}


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

/// Read `WTSINFOEXW.Data.WTSInfoExLevel1.SessionFlags` for this session.
///
/// Deliberately not cached: a stale "unlocked" would let proximity auto-lock
/// skip a lock it should have done.
fn query_session_locked() -> Option<bool> {
    use windows::Win32::System::RemoteDesktop::{
        WTSFreeMemory, WTSQuerySessionInformationW, WTSSessionInfoEx, WTSINFOEXW,
        WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK,
    };

    let mut buffer: windows::core::PWSTR = windows::core::PWSTR::null();
    let mut bytes: u32 = 0;
    // SAFETY: the out-params are ours; on success WTS allocates `buffer` and we
    // free it below on every path. The size check before the cast is what makes
    // the read safe — a shorter buffer would mean a different info level.
    let ok = unsafe {
        WTSQuerySessionInformationW(
            Some(WTS_CURRENT_SERVER_HANDLE),
            WTS_CURRENT_SESSION,
            WTSSessionInfoEx,
            &mut buffer,
            &mut bytes,
        )
    };
    if ok.is_err() || buffer.is_null() {
        return None;
    }
    let flags = if (bytes as usize) >= std::mem::size_of::<WTSINFOEXW>() {
        let info = unsafe { &*(buffer.0 as *const WTSINFOEXW) };
        // Level 1 is the only level this call returns; anything else means the
        // OS handed back something we don't understand, so say "don't know".
        if info.Level == 1 {
            Some(unsafe { info.Data.WTSInfoExLevel1 }.SessionFlags)
        } else {
            None
        }
    } else {
        None
    };
    unsafe { WTSFreeMemory(buffer.0 as *mut std::ffi::c_void) };
    // WTS_SESSIONSTATE_LOCK / _UNLOCK are reported the right way round on
    // Windows 8 and later. (On Windows 7 they were inverted — a documented OS
    // bug. Vortex targets 10+, so this does not compensate for it; doing so
    // blindly would invert the answer on every supported version.)
    flags.map(|f| f == WTS_SESSIONSTATE_LOCK as i32)
}

pub struct WindowsSession;

impl SessionControl for WindowsSession {
    fn lock(&self) -> BoxFuture<Result<(), String>> {
        Box::pin(async move {
            // Inside the async block, not before it: a function returning a
            // future must not lock the screen just because someone built the
            // future. Nothing is held across an await here (there is none), so
            // the raw call is fine on any worker.
            //
            // SAFETY: no arguments, no out-params. Fails only if the calling
            // process lacks a visible window station (a service, say), which is
            // exactly what the error is for.
            unsafe { windows::Win32::System::Shutdown::LockWorkStation() }
                .map_err(|e| format!("LockWorkStation: {e}"))
        })
    }

    /// Windows has no programmatic unlock, by design — credentials must be
    /// presented to the LogonUI. Proximity auto-unlock is therefore Linux-only;
    /// [`SessionControl::can_unlock`] reports that so the UI can hide the
    /// setting instead of offering something that always fails.
    fn unlock(&self) -> BoxFuture<Result<(), String>> {
        Box::pin(async { Err("windows cannot unlock a session programmatically".to_string()) })
    }

    /// Query the session's lock flag directly.
    ///
    /// The obvious route — `WTSRegisterSessionNotification` and track
    /// `WTS_SESSION_LOCK`/`_UNLOCK` from process start — cannot answer before
    /// the first transition, which is the wrong answer for a daemon that starts
    /// while the screen is already locked. `WTSSessionInfoEx` reports the
    /// current flag instead, so the first call is as correct as the hundredth.
    ///
    /// `None` means "couldn't tell", never "unlocked": proximity auto-lock must
    /// not act on a guess.
    fn is_locked(&self) -> BoxFuture<Option<bool>> {
        Box::pin(async move { query_session_locked() })
    }

    fn can_unlock(&self) -> bool {
        false
    }
}
