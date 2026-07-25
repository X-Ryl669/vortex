//! BLE scanner per spec §5.2.
//!
//! The Vortex pairable advertisement fits the entire ADV_IND in a single
//! Service Data 128-bit AD (§5.1.1), so the scanner filters by inspecting
//! each discovered device's service_data map and looking for the Vortex
//! Service UUID. A custom monitor pattern would also work but the high-
//! level discovery API is simpler and portable across BlueZ versions.
//!
//! Filter pipeline (§5.2):
//!   1. Service Data contains the Vortex Service UUID
//!   2. Service Data payload exactly 10 bytes
//!   3. Version byte == 0x01
//!   4. Reserved flag bits zero, exactly one mode bit set
//!   5. Mode match (caller filters: pairable vs trusted-presence)
//!
//! Frames that fail any check are silently dropped.

use std::time::SystemTime;

use bluer::{
    Adapter, AdapterEvent, Address, Device, DeviceEvent, DeviceProperty, DiscoveryFilter,
    DiscoveryTransport, Result as BluerResult,
};
use futures::stream::{SelectAll, StreamExt};
use futures::{pin_mut, Stream};
use tracing::{debug, info};

use super::{AdvPayload, ADV_PAYLOAD_LEN, VORTEX_SERVICE_UUID};

/// One observed Vortex candidate.
#[derive(Debug, Clone)]
pub struct VortexCandidate {
    pub address: Address,
    pub rssi: Option<i16>,
    pub local_name: Option<String>,
    pub payload: AdvPayload,
    pub observed_at: SystemTime,
}

/// Run a BLE discovery filtered to Vortex Service Data.
///
/// Yields candidates that pass the §5.2 filter pipeline. The caller is
/// responsible for stopping the scan (drop the returned future).
///
/// **Why we also watch per-device PropertyChanged events:** on BlueZ,
/// service data often arrives in *two* observations. The first
/// `DeviceAdded` is fired off a bare ADV_IND that doesn't yet carry
/// the full Vortex service data; BlueZ then receives the SCAN_RSP /
/// extended payload and emits a `ServiceData` property change. Without
/// watching that change, the scanner would observe phones advertising
/// correctly and silently skip them. We re-parse on every ServiceData
/// update — the user-facing UI dedupes by `instance` so re-emitting is
/// safe, and presence-token rotations (trusted runtime) naturally
/// surface as fresh candidates.
pub async fn run_filtered_scan<F>(adapter: Adapter, mut on_candidate: F) -> BluerResult<()>
where
    F: FnMut(VortexCandidate),
{
    info!(
        adapter = %adapter.name(),
        service = %VORTEX_SERVICE_UUID,
        "starting Vortex BLE scan"
    );

    // Restrict discovery to the LE transport. Without this, BlueZ runs a
    // *general* (dual-mode) discovery on this Intel controller: each cycle
    // includes a BR/EDR Inquiry that takes a fixed ~10.24 s and hogs the
    // single shared radio. The Vortex LE beacon is found in ~1 s, but the
    // adapter stays `Discovering` for the whole inquiry, so the LE
    // connect that follows is starved until the inquiry ends — that was
    // the ~10 s `connect_ms` (confirmed via btmon: back-to-back 10.24 s
    // Inquiry Complete events). LE-only discovery has no inquiry at all,
    // so discovery stops instantly on abort and the connect runs on a free
    // radio. (We advertise/scan purely over LE; BR/EDR discovery was never
    // wanted here.)
    // Re-assert the LE-only transport (best effort). The daemon pins LE
    // globally at startup; this re-affirms it whenever the adapter is
    // idle (e.g. after the earbuds picker temporarily set Auto for a
    // BR/EDR scan). When a discovery session is already active bluer
    // returns `DiscoveryActive` — harmless, because that live session is
    // already the LE one we want. Either way we must NOT bail out, or the
    // scan would never run. See the startup pin in ui-tauri lib.rs for
    // why LE-only matters (a general discovery's ~10.24 s BR/EDR Inquiry
    // starves every LE connect).
    if let Err(e) = adapter
        .set_discovery_filter(DiscoveryFilter {
            transport: DiscoveryTransport::Le,
            ..Default::default()
        })
        .await
    {
        debug!("LE-only discovery filter not re-applied ({e}); proceeding");
    }

    let adapter_events = adapter.discover_devices().await?;
    pin_mut!(adapter_events);

    // One stream per known device's PropertyChanged events. Tagged
    // with the device address so the consumer can route updates back
    // to the right handle.
    let mut device_events: SelectAll<DeviceEventStream> = SelectAll::new();

    loop {
        tokio::select! {
            evt = adapter_events.next() => {
                let Some(evt) = evt else { break };
                if let AdapterEvent::DeviceAdded(address) = evt {
                    // First-look parse (handles devices that ALREADY
                    // have service data attached, e.g. the cache from
                    // a prior scan).
                    try_emit(&adapter, address, &mut on_candidate).await;
                    // Subscribe to property changes so we also see the
                    // case where service data arrives after this event.
                    if let Ok(device) = adapter.device(address) {
                        if let Ok(stream) = device.events().await {
                            device_events.push(tag_with_address(address, stream));
                        } else {
                            debug!(%address, "device.events() failed; only initial parse");
                        }
                    }
                }
                // Other AdapterEvent variants (DeviceRemoved, etc.):
                // bluer GC's the device handle for us; the per-device
                // stream will end naturally and SelectAll will drop it.
            }
            Some((address, DeviceEvent::PropertyChanged(prop))) = device_events.next() => {
                // ServiceData updates are the primary discriminator —
                // RSSI flips constantly and would emit duplicates.
                // Name also re-parses because BlueZ often reports the
                // SCAN_RSP (where Android publishes the BT alias) AFTER
                // the initial ADV_IND with ServiceData has already
                // landed. Without this, the first emit carries
                // local_name=None and the UI shows the generic label
                // forever; the second emit on a Name change gives the
                // UI a chance to fill in "Redmi 9" etc.
                if matches!(prop, DeviceProperty::ServiceData(_) | DeviceProperty::Name(_)) {
                    try_emit(&adapter, address, &mut on_candidate).await;
                }
            }
            else => break,
        }
    }

    Ok(())
}

type DeviceEventStream = std::pin::Pin<Box<dyn Stream<Item = (Address, DeviceEvent)> + Send>>;

fn tag_with_address(
    address: Address,
    stream: impl Stream<Item = DeviceEvent> + Send + 'static,
) -> DeviceEventStream {
    Box::pin(stream.map(move |e| (address, e)))
}

/// Attempt to decode the Vortex service data for `address`. Silently
/// no-ops if the device hasn't published service data yet or it
/// doesn't pass the §5.2 filter pipeline — the caller will see another
/// PropertyChanged event when BlueZ updates the cache.
async fn try_emit<F>(adapter: &Adapter, address: Address, on_candidate: &mut F)
where
    F: FnMut(VortexCandidate),
{
    let device: Device = match adapter.device(address) {
        Ok(d) => d,
        Err(err) => {
            debug!(?err, %address, "could not open device handle");
            return;
        }
    };

    let service_data = match device.service_data().await {
        Ok(Some(sd)) => sd,
        Ok(None) => return,
        Err(err) => {
            debug!(?err, %address, "service_data lookup failed");
            return;
        }
    };

    let Some(bytes) = service_data.get(&VORTEX_SERVICE_UUID) else {
        return;
    };
    if bytes.len() != ADV_PAYLOAD_LEN {
        debug!(%address, len = bytes.len(), "service data has wrong length");
        return;
    }

    match AdvPayload::decode(bytes) {
        Ok(payload) => {
            let rssi = device.rssi().await.ok().flatten();
            let local_name = device.name().await.ok().flatten();
            on_candidate(VortexCandidate {
                address,
                rssi,
                local_name,
                payload,
                observed_at: SystemTime::now(),
            });
        }
        Err(err) => debug!(?err, %address, "Vortex payload decode failed; dropping"),
    }
}

#[cfg(test)]
mod tests {
    /// The scanner module exposes only the type and the scan future.
    /// Behavioral validation lives in live device tests; the codec is
    /// covered by `super::tests` in `mod.rs`.
    #[test]
    fn smoke() {}

    #[allow(dead_code)]
    fn is_send<T: Send>() {}

    #[test]
    fn vortex_candidate_is_send() {
        is_send::<super::VortexCandidate>();
    }
}
