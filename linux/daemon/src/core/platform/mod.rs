//! The platform seam: everything the laptop side needs from the OS, expressed
//! as traits so a second OS can be added without touching feature logic.
//!
//! # Why this exists
//!
//! Until now every OS call was made inline against a Linux API — BlueZ over
//! D-Bus, logind, XDG directories, the freedesktop notification service — with
//! no `cfg(target_os)` anywhere in the tree. That is fine for one OS and
//! impossible for two. These traits are the boundary: **feature logic above,
//! OS below**. The rule is that nothing above this line names a Linux concept.
//!
//! # What is deliberately NOT here
//!
//! * **Storage.** [`crate::core::storage`] already has `IdentityStore` and
//!   `PeerStore`; a Windows Credential Manager implementation slots in beside
//!   `SecretServiceIdentityStore` with no new trait.
//! * **The wire protocol, crypto, framing, LAN and mDNS.** They are pure Rust
//!   and must stay byte-identical across platforms — the phone cannot tell the
//!   two laptops apart, and `shared/vectors/` exists to keep it that way.
//! * **Clipboard.** `arboard` already covers Linux and Windows.
//!
//! # Status
//!
//! The Windows implementations are stubs that name the API they will call. They
//! are compiled only on Windows, so they cannot break the Linux build; and the
//! Linux implementations delegate to the existing modules, so this file adds a
//! boundary without changing behaviour.
//!
//! **The daemon LIBRARY compiles for Windows.** Verify with:
//!
//! ```text
//! cargo check -p vortex-l3-daemon --lib --target x86_64-pc-windows-gnu
//! ```
//!
//! Two notes on that command. `--lib`, because `src/main.rs` is a Linux BLE CLI
//! harness and is not part of a Windows build (the product there is the Tauri
//! app). And `-gnu` rather than `-msvc`: an MSVC cross-check needs `lib.exe`,
//! which a Linux box does not have, so it dies in `cc-rs` before reaching our
//! code. The GNU target type-checks the same source.
//!
//! What compiles is the platform-neutral core: crypto, framing, the wire
//! protocol (`ble::frame`), LAN + mDNS, the pairing state machine, appstate,
//! the storage traits, and this seam. What is gated out — with the reason on
//! each `cfg` — is every direct BlueZ / D-Bus / PulseAudio / Secret Service
//! module.
//!
//! # The gates are not the port
//!
//! A `cfg(target_os = "linux")` on a module means "no Windows implementation
//! yet", not "not needed on Windows". Two of them are load-bearing and will
//! come back as trait work rather than as a second copy:
//!
//! * `pairing::{handshake, reconnect}` — the XX/IK Noise state machines are
//!   platform-neutral but written against `ble::client::VortexClient`
//!   concretely. They need to take `&dyn GattLink`, after which both platforms
//!   share them. This is the most security-critical path in the tree, so it was
//!   deliberately left out of the mechanical gating pass.
//! * `ble::audio_signal` — the frame dispatch, cipher-resync and AppState
//!   decode in there are protocol logic that happens to live inside the BlueZ
//!   notification listener. Windows needs the same dispatch behind a different
//!   transport, so this wants splitting rather than reimplementing.

use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "windows")]
pub mod windows;

/// Standard user directories. Localised on both platforms and NOT derivable by
/// joining an English folder name onto `$HOME` — the French desktop this was
/// first written on uses `~/Téléchargements`, and Windows relocates the
/// Downloads folder freely (OneDrive moves it by default).
pub trait UserPaths: Send + Sync {
    /// Where received files are saved. Must be the user's real download folder.
    fn downloads(&self) -> Option<PathBuf>;
    /// Per-user config root (`~/.config/vortex`, `%APPDATA%\Vortex`).
    fn config(&self) -> Option<PathBuf>;
    /// Per-user cache root — icon cache, transient blobs.
    fn cache(&self) -> Option<PathBuf>;
}

/// A desktop notification carrying optional action buttons.
///
/// The hard part on both platforms is not showing it, it is getting the click
/// back. On Linux the sender must stay on the bus for Plasma to keep the
/// buttons, while GNOME needs a windowless sender. On Windows the toast needs
/// an AppUserModelID, and an unpackaged app needs a registered COM activator
/// before an action can round-trip at all.
pub trait Notifier: Send + Sync {
    /// Show (or replace, when `replaces` is non-zero) a notification. `actions`
    /// is `(key, label)`; the key comes back through [`Notifier::actions`].
    fn show(
        &self,
        summary: &str,
        body: &str,
        app_id: &str,
        actions: &[(String, String)],
        replaces: u32,
        urgent: bool,
    ) -> BoxFuture<Result<u32, String>>;

    /// Withdraw a notification we previously showed.
    fn close(&self, id: u32) -> BoxFuture<Result<(), String>>;

    /// Stream of `(notification id, action key)` for every button the user
    /// clicks. One process-wide stream: consumers filter by key prefix
    /// (`fc:` file consent, `call:` call banner, `act:` mirrored action).
    fn actions(&self, tx: tokio::sync::mpsc::UnboundedSender<(u32, String)>);

    /// Stream of `(notification id, reason)` closures, so a dismissal on the
    /// laptop can be mirrored back to the phone.
    fn closures(&self, tx: tokio::sync::mpsc::UnboundedSender<(u32, u32)>);
}

/// Lock / unlock the desktop session and report its current state — the
/// proximity feature's entire OS surface.
///
/// Windows can lock (`LockWorkStation`) but deliberately cannot unlock
/// programmatically, so proximity *auto-unlock* is Linux-only and the trait
/// lets an implementation say so rather than fail at the call site.
pub trait SessionControl: Send + Sync {
    fn lock(&self) -> BoxFuture<Result<(), String>>;
    fn unlock(&self) -> BoxFuture<Result<(), String>>;
    /// `None` when the platform can't report it.
    fn is_locked(&self) -> BoxFuture<Option<bool>>;
    /// Whether [`SessionControl::unlock`] can work at all here.
    fn can_unlock(&self) -> bool;
}

/// Run Vortex at login.
pub trait Autostart: Send + Sync {
    fn is_enabled(&self) -> bool;
    fn set_enabled(&self, on: bool) -> Result<(), String>;
}

/// Pointer/keyboard capture for Universal Control: hold the cursor at a screen
/// edge, take exclusive input, and stream events for forwarding to the phone.
///
/// This is the one subsystem that is *easier* on Windows — a low-level hook
/// plus `ClipCursor` does what Wayland needs the input-capture portal and libei
/// for, and it works the same on every Windows desktop.
pub trait InputCapture: Send + Sync {
    /// Arm capture on the given edge. Events flow to `tx` until released.
    fn arm(&self, edge: Edge, tx: tokio::sync::mpsc::UnboundedSender<InputEvent>)
        -> BoxFuture<Result<(), String>>;
    /// Release capture; the cursor returns to the laptop.
    fn release(&self) -> BoxFuture<Result<(), String>>;
    /// Hide the laptop's own cursor while control is on the phone. Best-effort:
    /// GNOME-only today, and `false` means "couldn't", not "failed".
    fn hide_cursor(&self, hidden: bool) -> bool;
}

/// Which screen edge the phone sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

/// One captured input event, already in the platform-neutral form the phone
/// side expects.
#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    /// Relative pointer motion.
    Motion { dx: f64, dy: f64 },
    /// Button 1 = left, 2 = middle, 3 = right.
    Button { button: u8, pressed: bool },
    /// Vertical / horizontal scroll, in notches.
    Scroll { dx: f64, dy: f64 },
    /// A Linux evdev keycode — the phone side already speaks these, so Windows
    /// translates its VK codes into this space rather than inventing a third.
    Key { keycode: u16, pressed: bool },
}

/// BLE central role: scan for the phone's advertisement, connect, and talk GATT.
///
/// The laptop is central-**only** — it never advertises and never serves a GATT
/// server, which is what makes Windows viable at all (WinRT's peripheral role is
/// far weaker than its central role).
pub trait BleCentral: Send + Sync {
    /// Scan until a Vortex advertisement is seen, or the timeout elapses.
    fn scan_for_peer(&self, timeout_ms: u64) -> BoxFuture<Result<Option<PeerAddr>, String>>;
    /// Connect and resolve the Vortex GATT service.
    fn connect(&self, addr: PeerAddr) -> BoxFuture<Result<Box<dyn GattLink>, String>>;
    /// Addresses of already-bonded devices, for the reconnect fast path.
    fn bonded(&self) -> BoxFuture<Result<Vec<PeerAddr>, String>>;
    /// Whether the radio is present and powered.
    fn adapter_ready(&self) -> BoxFuture<bool>;
}

/// An open GATT connection to the phone.
pub trait GattLink: Send + Sync {
    /// Write one frame to a characteristic (`with_response` = acknowledged).
    fn write(&self, char_uuid: Uuid128, data: &[u8], with_response: bool)
        -> BoxFuture<Result<(), String>>;
    /// Subscribe to notifications; frames arrive on `tx` until disconnect.
    fn subscribe(&self, char_uuid: Uuid128, tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>)
        -> BoxFuture<Result<(), String>>;
    fn disconnect(&self) -> BoxFuture<Result<(), String>>;
    fn is_connected(&self) -> bool;
}

/// A Bluetooth device address. Deliberately a plain newtype rather than
/// `bluer::Address`: Windows hands out a `u64`, and the resolvable private
/// addresses the phone rotates through mean the *value* is never a stable
/// identity anyway — the peer's static public key is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PeerAddr(pub [u8; 6]);

impl PeerAddr {
    /// From the `u64` WinRT hands out (`BluetoothLEDevice::BluetoothAddress`,
    /// `BluetoothLEAdvertisementReceivedEventArgs::BluetoothAddress`).
    ///
    /// The 48-bit address occupies the low six bytes, most-significant byte
    /// first in printed order: `0x0000_AABB_CCDD_EEFF` is `AA:BB:CC:DD:EE:FF`.
    /// The top two bytes are always zero and are dropped. Kept here rather than
    /// in the Windows module so it can be tested on either platform — it is
    /// pure arithmetic, and getting it backwards would mean connecting to a
    /// mirrored address that simply never answers.
    pub fn from_u48(addr: u64) -> Self {
        let b = addr.to_be_bytes();
        Self([b[2], b[3], b[4], b[5], b[6], b[7]])
    }

    /// The inverse of [`PeerAddr::from_u48`], for handing an address back to a
    /// WinRT call.
    pub fn to_u48(self) -> u64 {
        let a = self.0;
        u64::from_be_bytes([0, 0, a[0], a[1], a[2], a[3], a[4], a[5]])
    }
}

impl std::fmt::Display for PeerAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let b = self.0;
        write!(
            f,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        )
    }
}

/// A 128-bit GATT UUID, in the same byte order both platforms accept.
pub type Uuid128 = u128;

/// Boxed future alias — these traits are object-safe on purpose (the active
/// platform is chosen at runtime through `dyn`, so feature code never carries a
/// platform type parameter).
pub type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

/// The active platform's user paths.
pub fn paths() -> &'static dyn UserPaths {
    #[cfg(target_os = "linux")]
    {
        &linux::LinuxPaths
    }
    #[cfg(target_os = "windows")]
    {
        &windows::WindowsPaths
    }
}

/// The active platform's notifier.
pub fn notifier() -> &'static dyn Notifier {
    #[cfg(target_os = "linux")]
    {
        &linux::LinuxNotifier
    }
    #[cfg(target_os = "windows")]
    {
        &windows::WindowsNotifier
    }
}

/// The active platform's session control.
pub fn session() -> &'static dyn SessionControl {
    #[cfg(target_os = "linux")]
    {
        &linux::LinuxSession
    }
    #[cfg(target_os = "windows")]
    {
        &windows::WindowsSession
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The byte order is a wire-level detail with no error path: a mirrored
    /// address is a valid-looking address that nothing answers on.
    #[test]
    fn u48_round_trips_and_keeps_printed_order() {
        let a = PeerAddr([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        assert_eq!(a.to_u48(), 0x0000_AABB_CCDD_EEFF);
        assert_eq!(PeerAddr::from_u48(0x0000_AABB_CCDD_EEFF), a);
        assert_eq!(a.to_string(), "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn the_high_two_bytes_are_dropped() {
        // WinRT always zeroes them; be explicit that we don't fold them in.
        assert_eq!(
            PeerAddr::from_u48(0xFFFF_0011_2233_4455),
            PeerAddr([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]),
        );
    }

    #[test]
    fn a_random_address_survives_a_round_trip() {
        for seed in [0u64, 1, 0x0000_0102_0304_0506, 0x0000_FFFF_FFFF_FFFF] {
            assert_eq!(PeerAddr::from_u48(seed).to_u48(), seed);
        }
    }
}
