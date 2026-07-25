//! Locally-connected Bluetooth audio detection.
//!
//! We treat any BlueZ device that is currently connected AND advertises
//! at least one classic Bluetooth audio profile (A2DP / HFP / HSP) as
//! "the user's earbuds". The first such device wins for V1 — multipoint
//! / multi-headset selection lands in a later phase.
//!
//! Battery percentage is read via the `org.bluez.Battery1` interface
//! when the device exposes it. Most modern earbuds (AirPods Pro,
//! Galaxy Buds, Sony WF-*, etc.) report it.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use bluer::{Adapter, Address, DiscoveryFilter, DiscoveryTransport};
use uuid::Uuid;

use crate::core::appstate::EarbudsInfo;
use crate::core::earbuds_store;

/// Last-known battery percentage per earbuds address. BlueZ stops
/// exposing `org.bluez.Battery1` the moment the buds disconnect (e.g.
/// mid call-handoff switch, when they jump to the phone for a few
/// seconds) and also takes a beat to re-read it after a reconnect — so
/// a fresh render would briefly show a blank battery. We hold the last
/// value we ever saw and fall back to it, so the card keeps showing the
/// previous percentage through the gap. It refreshes to the real value
/// on the next poll once BlueZ reports it again.
fn battery_cache() -> &'static Mutex<HashMap<String, u8>> {
    static CACHE: OnceLock<Mutex<HashMap<String, u8>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Merge a fresh BlueZ reading with the cache: a `Some` updates the
/// cache and wins; a `None` falls back to the last-known value (if any).
fn battery_with_fallback(addr: &str, fresh: Option<u8>) -> Option<u8> {
    let mut cache = match battery_cache().lock() {
        Ok(c) => c,
        Err(p) => p.into_inner(),
    };
    match fresh {
        Some(pct) => {
            cache.insert(addr.to_string(), pct);
            Some(pct)
        }
        None => cache.get(addr).copied(),
    }
}

/// Service UUIDs that indicate this is an audio device. Sink/source
/// either side of A2DP plus the HFP / HSP variants.
const AUDIO_SERVICE_UUIDS: &[&str] = &[
    "0000110a-0000-1000-8000-00805f9b34fb", // A2DP Source
    "0000110b-0000-1000-8000-00805f9b34fb", // A2DP Sink
    "0000110d-0000-1000-8000-00805f9b34fb", // A2DP Advanced Audio Distribution
    "0000110e-0000-1000-8000-00805f9b34fb", // AVRCP Controller
    "0000111e-0000-1000-8000-00805f9b34fb", // HFP HF
    "0000111f-0000-1000-8000-00805f9b34fb", // HFP AG
    "00001108-0000-1000-8000-00805f9b34fb", // HSP HS
    "00001112-0000-1000-8000-00805f9b34fb", // HSP AG
];

fn audio_uuid_set() -> HashSet<Uuid> {
    AUDIO_SERVICE_UUIDS
        .iter()
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect()
}

/// Resolve the earbuds slot shown on the home screen.
///
/// Saved-entry only: the card represents the device the user explicitly
/// picked through the in-app modal. We no longer auto-detect the first
/// currently-connected audio peripheral — that turned "Remove from
/// Vortex" into a no-op, since the system bond stayed and the card
/// just re-appeared on the next poll. Returning None when nothing is
/// saved lets the UI render the "+ Add earbuds" placeholder cleanly.
pub async fn scan_local_earbuds(adapter: &Adapter) -> Option<EarbudsInfo> {
    let saved = earbuds_store::load()?;
    Some(resolve_saved(adapter, &saved).await)
}

async fn resolve_saved(adapter: &Adapter, saved: &earbuds_store::SavedEarbuds) -> EarbudsInfo {
    let addr: Address = match saved.address.parse() {
        Ok(a) => a,
        Err(_) => return offline_card(saved),
    };
    let device = match adapter.device(addr) {
        Ok(d) => d,
        Err(_) => return offline_card(saved),
    };
    let connected = device.is_connected().await.unwrap_or(false);
    let fresh = device.battery_percentage().await.ok().flatten();
    // Keep showing the last-known battery through a disconnect/reconnect
    // gap (the call-handoff switch) instead of blanking the card. If the
    // real value changed while the buds were away, the next poll picks it
    // up once BlueZ re-exposes Battery1.
    let battery = battery_with_fallback(&saved.address, fresh);
    let name = device
        .name()
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| saved.name.clone());
    EarbudsInfo {
        name,
        address: saved.address.clone(),
        battery,
        connected,
    }
}

fn offline_card(saved: &earbuds_store::SavedEarbuds) -> EarbudsInfo {
    EarbudsInfo {
        name: saved.name.clone(),
        address: saved.address.clone(),
        // Even with no live device handle, show the last battery we saw.
        battery: battery_with_fallback(&saved.address, None),
        connected: false,
    }
}

/// First-run helper: find a Bluetooth audio device that is ALREADY
/// connected to this laptop, so we can adopt it as the user's earbuds
/// without making them open the picker. `list_known_devices` already
/// sorts audio-class first, then connected, then by RSSI — so the first
/// entry that is both audio and connected is the best candidate. Returns
/// None when nothing audio-class is currently connected.
///
/// Callers gate this on `earbuds_store::autodetect_done()` so it only
/// fires once per install (see the setup hook), keeping "Remove from
/// Vortex" sticky.
pub async fn detect_connected_earbud(adapter: &Adapter) -> Option<earbuds_store::SavedEarbuds> {
    list_known_devices(adapter)
        .await
        .into_iter()
        .find(|d| d.is_audio && d.connected)
        .map(|d| earbuds_store::SavedEarbuds {
            address: d.address,
            name: d.name,
        })
}

/// One discoverable Bluetooth device, surfaced by the in-app
/// "+ Add earbuds" modal so the user can pick one to save.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BluetoothDevice {
    pub address: String,
    pub name: String,
    pub rssi: Option<i16>,
    pub connected: bool,
    pub is_audio: bool,
}

/// List every Bluetooth device BlueZ knows about right now (paired +
/// previously-seen + freshly discovered). UI uses this to populate
/// the device picker. Audio-class devices are flagged so the modal
/// can sort them to the top.
pub async fn list_known_devices(adapter: &Adapter) -> Vec<BluetoothDevice> {
    let audio = audio_uuid_set();
    let addresses = match adapter.device_addresses().await {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::with_capacity(addresses.len());
    for addr in addresses {
        let device = match adapter.device(addr) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let name = device
            .name()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| addr.to_string());
        let rssi = device.rssi().await.ok().flatten();
        let connected = device.is_connected().await.unwrap_or(false);
        let uuids: HashSet<Uuid> = device.uuids().await.ok().flatten().unwrap_or_default();
        let is_audio = !uuids.is_disjoint(&audio);
        out.push(BluetoothDevice {
            address: addr.to_string(),
            name,
            rssi,
            connected,
            is_audio,
        });
    }
    // Audio devices first, then connected ones, then by stronger RSSI.
    out.sort_by(|a, b| {
        b.is_audio
            .cmp(&a.is_audio)
            .then_with(|| b.connected.cmp(&a.connected))
            .then_with(|| b.rssi.unwrap_or(-127).cmp(&a.rssi.unwrap_or(-127)))
    });
    out
}

/// Kick off a short BlueZ device-discovery so fresh, never-seen
/// peripherals show up in [`list_known_devices`]. Best-effort.
pub async fn start_brief_discovery(adapter: &Adapter, duration: std::time::Duration) {
    use futures::StreamExt;
    // Explicitly request a dual-mode discovery: earbuds are BR/EDR (A2DP)
    // peripherals, so we MUST include the Classic transport here. The
    // Vortex LE scan sets an LE-only discovery filter (to dodge the ~10 s
    // BR/EDR Inquiry — see ble/scanner.rs), and BlueZ's SetDiscoveryFilter
    // is sticky per-client, so without re-asserting Auto here this
    // earbuds discovery could inherit LE-only and never surface the buds.
    let _ = adapter
        .set_discovery_filter(DiscoveryFilter {
            transport: DiscoveryTransport::Auto,
            ..Default::default()
        })
        .await;
    if let Ok(stream) = adapter.discover_devices().await {
        let _ = tokio::time::timeout(duration, async move {
            futures::pin_mut!(stream);
            while stream.next().await.is_some() {
                // Each event already updates BlueZ's internal device
                // table — we just need to keep the stream open long
                // enough for that to happen.
            }
        })
        .await;
    }
    // RESTORE the LE-only filter — SetDiscoveryFilter is sticky per
    // D-Bus session, so leaving Auto behind poisons every later scan on
    // this adapter handle: whenever a scan's own SetDiscoveryFilter(Le)
    // loses the Busy race, its StartDiscovery falls back to the STICKY
    // filter, and Auto there means a back-to-back ~10.24s BR/EDR
    // Inquiry loop that hogs the radio and stretches every LE connect
    // to ~10.5s (live-confirmed via btmon during the proximity-unlock
    // latency hunt).
    let _ = adapter
        .set_discovery_filter(DiscoveryFilter {
            transport: DiscoveryTransport::Le,
            ..Default::default()
        })
        .await;
}
