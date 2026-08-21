//! Pairing orchestration per spec §6.

pub mod backoff;

// The XX pairing and IK reconnect handshakes as run OVER BLE: the Noise state
// machine here is platform-neutral, but these two are written against
// `ble::client::VortexClient` concretely, so they can't build without BlueZ.
//
// Porting them is not a matter of cfg: they need to take a
// `&dyn core::platform::GattLink` instead of a `&VortexClient`, at which point
// both platforms share this code and only the transport differs. That refactor
// touches the most security-critical path in the tree, so it is deliberately
// NOT bundled with the mechanical gating — see the note in `platform`.
//
// The LAN side is unaffected either way: `lan::tcp_client` runs its own IK over
// TCP and is already portable.
#[cfg(target_os = "linux")]
pub mod handshake;
#[cfg(target_os = "linux")]
pub mod reconnect;
