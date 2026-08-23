//! The BLE persistent link, written against the platform seam.
//!
//! Same job as `ble::run_ble_persistent_loop`: keep a GATT link to the phone
//! up, run Noise IK over it, publish the laptop→phone writers, and pump the
//! event stream until it drops. The difference is what it talks to — only
//! [`BleCentral`] and [`GattLink`], so it runs anywhere the seam has an
//! implementation.
//!
//! # Why this exists beside the Linux loop rather than replacing it
//!
//! The BlueZ loop has years of hard-won behaviour that has no counterpart in
//! the seam and no meaning off Linux: an adapter power-cycle self-heal for a
//! wedged stack, `remove_device` to force a re-resolve, learning the phone's
//! last RPA to skip a scan, and a bearer dance for dual-mode phones. Rewriting
//! that against the seam would mean either dropping it or inventing seam
//! methods that exist for one OS. So Linux keeps its loop, and this is the
//! portable one — which is also a fair description of the next refactor, once
//! the Windows side has run long enough to say which of those behaviours are
//! BlueZ quirks and which are BLE ones.
//!
//! # Untested
//!
//! Never run. The pieces underneath it are: the IK handshake and the event
//! stream both have unit tests against `FakeGattLink`. What is unexercised is
//! this orchestration and the WinRT transport below it.

use std::sync::Arc;

use tokio::sync::Mutex;

use vortex_l3_daemon::core::ble::audio_signal;
use vortex_l3_daemon::core::crypto::noise::TransportState;
use vortex_l3_daemon::core::identity::IdentityRecord;
use vortex_l3_daemon::core::pairing::reconnect::run_ik_initiator;
use vortex_l3_daemon::core::platform::{BleCentral, GattLink};
use vortex_l3_daemon::core::storage::peers::PeerStore;

/// Where the phone's event stream is delivered. One field per frame type the
/// listener dispatches; see `audio_signal::run_listener` for what each carries.
///
/// A struct rather than 14 positional arguments — the listener's own signature
/// predates the seam and is what it is, but nothing forces this side to repeat
/// it.
pub(crate) struct BleSinks {
    pub state: tokio::sync::mpsc::UnboundedSender<(
        [u8; 32],
        vortex_l3_daemon::core::appstate::AppState,
    )>,
    pub notif: tokio::sync::mpsc::UnboundedSender<
        vortex_l3_daemon::core::notif_mirror::NotificationMirror,
    >,
    pub live: tokio::sync::mpsc::UnboundedSender<
        vortex_l3_daemon::core::live_activity::LiveActivity,
    >,
    pub icon: tokio::sync::mpsc::UnboundedSender<(String, u16, u16, Vec<u8>)>,
    pub call: tokio::sync::mpsc::UnboundedSender<vortex_l3_daemon::core::call_event::CallEvent>,
    pub contacts: tokio::sync::mpsc::UnboundedSender<(u16, u16, Vec<u8>)>,
    pub call_log: tokio::sync::mpsc::UnboundedSender<(u16, u16, Vec<u8>)>,
    pub sms: tokio::sync::mpsc::UnboundedSender<(u16, u16, Vec<u8>)>,
    pub sms_thread: tokio::sync::mpsc::UnboundedSender<(u16, u16, Vec<u8>)>,
    pub clipboard: tokio::sync::mpsc::UnboundedSender<
        vortex_l3_daemon::core::clipboard_mirror::ClipboardMirror,
    >,
    pub clipboard_image: tokio::sync::mpsc::UnboundedSender<(u16, u16, Vec<u8>)>,
    pub clipboard_offer: tokio::sync::mpsc::UnboundedSender<
        vortex_l3_daemon::core::clipboard_mirror::ClipboardImageOffer,
    >,
    pub handoff: tokio::sync::mpsc::UnboundedSender<
        vortex_l3_daemon::core::handoff::HandoffEvent,
    >,
    pub raw: tokio::sync::mpsc::UnboundedSender<(u8, Vec<u8>)>,
}

/// The laptop→phone writer holders the features send through. Filled on connect,
/// cleared on disconnect, so a feature that fires with no link gets `None`
/// rather than a write into a dead socket.
pub(crate) struct BleWriterSlots {
    pub notif: Arc<Mutex<Option<crate::NotifWriter>>>,
    pub clipboard: Arc<Mutex<Option<crate::ClipboardWriter>>>,
    pub clipboard_image: Arc<Mutex<Option<crate::ClipboardImageWriter>>>,
    pub call: Arc<Mutex<Option<crate::CallWriter>>>,
    pub sealed: Arc<Mutex<Option<crate::SealedWriter>>>,
}

/// Reconnect backoff. Deliberately short at the start: a walk-out-of-range and
/// back is the common case, and the phone is usually there again within
/// seconds. Capped so a phone that is off overnight isn't scanned for
/// continuously.
const BACKOFF_SECS: [u64; 5] = [2, 5, 15, 30, 60];

/// How long each scan runs before giving up and backing off. Long enough to
/// catch the phone's advertising interval a few times over.
const SCAN_MS: u64 = 8_000;

/// Keep a BLE link to the trusted peer up, forever.
///
/// Returns only if there is no way to proceed at all; every transient failure
/// is a backoff and a retry. `retry_nudge` wakes it early — the LAN heartbeat
/// fires that when it sees the phone appear on the network, which is a strong
/// hint the radio is in range too.
pub(crate) async fn run_portable_ble_loop(
    central: Arc<dyn BleCentral>,
    identity: IdentityRecord,
    peer_store: Arc<dyn PeerStore>,
    sinks: BleSinks,
    writers: BleWriterSlots,
    retry_nudge: Arc<tokio::sync::Notify>,
) {
    let mut consec_fail: usize = 0;
    // Announce itself once. The wait-for-a-peer branch below is silent by
    // design (it polls every 10 s and would flood), but that made a first run
    // with nothing paired indistinguishable from a loop that never started —
    // which is how the first Windows log read.
    tracing::info!("portable BLE loop started");
    let mut announced_no_peer = false;
    loop {
        // A trusted peer is the precondition for everything below: IK needs the
        // peer's static key and the PRS. Before pairing there is nothing to
        // connect to, so wait rather than scan.
        let peer = {
            let store = peer_store.clone();
            match tokio::task::spawn_blocking(move || {
                store.list().unwrap_or_default().into_iter().next()
            })
            .await
            {
                Ok(Some(p)) => {
                    announced_no_peer = false;
                    p
                }
                _ => {
                    if !announced_no_peer {
                        tracing::info!("no trusted peer yet; BLE loop idle until pairing");
                        announced_no_peer = true;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    continue;
                }
            }
        };

        if !central.adapter_ready().await {
            tracing::debug!("BLE radio not ready; waiting");
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            continue;
        }

        match connect_and_run(&central, &identity, &peer_store, &peer, &sinks, &writers).await {
            Ok(()) => {
                // A clean return means the link dropped, which is normal — the
                // phone moved, slept, or restarted. Reconnect promptly.
                tracing::info!("BLE link closed; reconnecting");
                consec_fail = 0;
            }
            Err(e) => {
                tracing::info!("BLE attempt failed: {e}");
                consec_fail = consec_fail.saturating_add(1);
            }
        }

        // Clear the writers before waiting: a feature firing during the gap
        // must see "no link" rather than push into a torn-down one.
        clear_writers(&writers).await;

        let wait = BACKOFF_SECS[consec_fail.min(BACKOFF_SECS.len() - 1)];
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(wait)) => {}
            _ = retry_nudge.notified() => {
                tracing::info!("BLE retry nudged early (phone seen on the network)");
            }
        }
    }
}

/// One connect → IK → listen cycle. Returns `Ok` when the link closes normally.
async fn connect_and_run(
    central: &Arc<dyn BleCentral>,
    identity: &IdentityRecord,
    peer_store: &Arc<dyn PeerStore>,
    peer: &vortex_l3_daemon::core::storage::peers::TrustedPeer,
    sinks: &BleSinks,
    writers: &BleWriterSlots,
) -> Result<(), String> {
    // Bonded addresses first: a paired phone is usually reachable at an address
    // the OS already knows, which skips the scan entirely. The phone rotates
    // its advertising address, so this can be stale — hence the fallback rather
    // than a failure.
    let mut candidates = central.bonded().await.unwrap_or_default();
    if candidates.is_empty() {
        match central.scan_for_peer(SCAN_MS).await? {
            Some(found) => {
                tracing::info!(
                    addr = %found.addr,
                    rssi = ?found.rssi,
                    pairable = found.payload.flags.is_pairable(),
                    "BLE: Vortex advertisement seen"
                );
                candidates.push(found.addr);
            }
            None => return Err("no Vortex advertisement in range".to_string()),
        }
    }

    // Try each in turn: `bonded()` can list several devices, and only one of
    // them answers our service.
    let mut last_err = "no candidate answered".to_string();
    for addr in candidates {
        let link: Box<dyn GattLink> = match central.connect(addr).await {
            Ok(l) => l,
            Err(e) => {
                last_err = format!("{addr}: {e}");
                continue;
            }
        };
        let link: Arc<dyn GattLink> = Arc::from(link);

        let local_counter = {
            let store = peer_store.clone();
            let peer_pub = peer.peer_static_pub;
            tokio::task::spawn_blocking(move || store.load_counter(&peer_pub).unwrap_or(0))
                .await
                .unwrap_or(0)
        };

        let outcome = match run_ik_initiator(
            &*link,
            &identity.static_priv.0,
            &peer.peer_static_pub,
            &peer.prs,
            local_counter,
            std::time::Duration::from_secs(10),
        )
        .await
        {
            Ok(o) => o,
            Err(e) => {
                // A device that answered GATT but failed IK is not our peer, or
                // its trust record no longer matches ours. Either way, move on.
                last_err = format!("{addr}: IK failed: {e}");
                continue;
            }
        };
        if outcome.peer_counter < local_counter {
            tracing::warn!(
                "possible trust rollback: peer counter={} local={}",
                outcome.peer_counter,
                local_counter
            );
        }
        {
            let store = peer_store.clone();
            let peer_pub = peer.peer_static_pub;
            let seen = outcome.peer_counter;
            tokio::spawn(async move {
                let _ = store.bump_counter(&peer_pub, seen);
            });
        }

        let transport = match outcome.transport {
            Some(t) => Arc::new(Mutex::new(t)),
            // The IK entrypoint always returns one; a `None` would mean the
            // handshake succeeded without producing ciphers, which cannot
            // happen and must not be papered over with a second handshake.
            None => return Err("IK produced no transport ciphers".to_string()),
        };

        tracing::info!(addr = %addr, "BLE link established");
        crate::presence::touch_presence();
        crate::presence::touch_peer_contact();
        publish_writers(&link, &transport, writers).await;

        // Runs until the phone stops notifying — a disconnect, or the cipher
        // desync escalation dropping the session on purpose.
        let r = audio_signal::run_listener(
            &*link,
            transport,
            peer.peer_static_pub,
            // No audio backend: AUDIO_OP frames are dropped and the other
            // eighteen types carry on. See `platform::AudioHandoff`.
            None,
            Some(sinks.state.clone()),
            Some(sinks.notif.clone()),
            Some(sinks.live.clone()),
            Some(sinks.icon.clone()),
            Some(sinks.call.clone()),
            Some(sinks.contacts.clone()),
            Some(sinks.call_log.clone()),
            Some(sinks.sms.clone()),
            Some(sinks.sms_thread.clone()),
            Some(sinks.clipboard.clone()),
            Some(sinks.clipboard_image.clone()),
            Some(sinks.clipboard_offer.clone()),
            Some(sinks.handoff.clone()),
            Some(sinks.raw.clone()),
        )
        .await;
        let _ = link.disconnect().await;
        return match r {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("listener: {e}")),
        };
    }
    Err(last_err)
}

/// Fill the writer slots for this live link.
async fn publish_writers(
    link: &Arc<dyn GattLink>,
    transport: &Arc<Mutex<TransportState>>,
    writers: &BleWriterSlots,
) {
    {
        let t = transport.clone();
        let l = link.clone();
        let w: crate::NotifWriter = Arc::new(move |notif| {
            let (t, l) = (t.clone(), l.clone());
            Box::pin(async move { audio_signal::write_notification(&*l, t, &notif).await })
        });
        *writers.notif.lock().await = Some(w);
    }
    {
        let t = transport.clone();
        let l = link.clone();
        let w: crate::ClipboardWriter = Arc::new(move |clip| {
            let (t, l) = (t.clone(), l.clone());
            Box::pin(async move { audio_signal::write_clipboard(&*l, t, &clip).await })
        });
        *writers.clipboard.lock().await = Some(w);
    }
    {
        let t = transport.clone();
        let l = link.clone();
        let w: crate::ClipboardImageWriter = Arc::new(move |png| {
            let (t, l) = (t.clone(), l.clone());
            Box::pin(async move { audio_signal::write_clipboard_image(&*l, t, &png).await })
        });
        *writers.clipboard_image.lock().await = Some(w);
    }
    {
        let t = transport.clone();
        let l = link.clone();
        let w: crate::CallWriter = Arc::new(move |ctrl| {
            let (t, l) = (t.clone(), l.clone());
            Box::pin(async move { audio_signal::write_call_control(&*l, t, &ctrl).await })
        });
        *writers.call.lock().await = Some(w);
    }
    {
        let t = transport.clone();
        let l = link.clone();
        let w: crate::SealedWriter = Arc::new(move |ty, payload| {
            let (t, l) = (t.clone(), l.clone());
            Box::pin(async move { audio_signal::write_sealed(&*l, t, ty, &payload).await })
        });
        *writers.sealed.lock().await = Some(w);
    }
}

async fn clear_writers(writers: &BleWriterSlots) {
    *writers.notif.lock().await = None;
    *writers.clipboard.lock().await = None;
    *writers.clipboard_image.lock().await = None;
    *writers.call.lock().await = None;
    *writers.sealed.lock().await = None;
}

/// Pair with a phone that has its pairing window open, over the seam.
///
/// Scans rather than taking an address. On Linux the UI lists scanned devices
/// and the user picks one, because BlueZ hands us a device list for free; here
/// the seam's scan answers "is there a Vortex phone advertising as pairable",
/// which is the question the user is actually asking when they press Pair. If
/// two phones are in a pairing window at once this takes the first seen — the
/// SAS comparison is what makes that safe, since pairing with the wrong phone
/// shows a code that does not match and the user rejects it.
pub(crate) async fn pair_by_scan(
    app: &tauri::AppHandle,
    central: Arc<dyn BleCentral>,
    identity: &IdentityRecord,
    peer_store: Arc<dyn PeerStore>,
) -> Result<(), String> {
    if !central.adapter_ready().await {
        return Err("no Bluetooth LE radio available".to_string());
    }
    // Longer than the reconnect scan: the user has just pressed Pair on both
    // devices and is watching, so it is worth waiting for the phone's window to
    // come up rather than failing and making them press it again.
    let found = central
        .scan_for_peer(20_000)
        .await?
        .ok_or_else(|| "no phone advertising a pairing window was found".to_string())?;
    if !found.payload.flags.is_pairable() {
        // A trusted-presence advert means that phone is already paired with
        // someone — possibly us. Saying so beats a confusing handshake failure.
        return Err(
            "the phone found is not in pairing mode — open Vortex on the phone and tap Pair"
                .to_string(),
        );
    }
    tracing::info!(addr = %found.addr, "pairing: pairable phone found");

    let link = central.connect(found.addr).await?;

    // Capability read first, per §9.1.5: it is the version check, and doing it
    // before the handshake means a version mismatch is reported as such instead
    // of surfacing as an AEAD failure mid-XX.
    match link
        .read(vortex_l3_daemon::core::ble::CAPABILITY_UUID.as_u128())
        .await
    {
        Ok(bytes) if bytes.first() == Some(&vortex_l3_daemon::core::ble::V1_VERSION) => {}
        Ok(bytes) => {
            return Err(format!(
                "phone speaks protocol version {:?}, this build speaks {}",
                bytes.first(),
                vortex_l3_daemon::core::ble::V1_VERSION
            ))
        }
        Err(e) => return Err(format!("capability read failed: {e}")),
    }

    crate::pairing::do_pair_over(
        app,
        &*link,
        identity,
        peer_store,
        crate::pairing::local_device_name().as_deref(),
    )
    .await
}

/// `UiCmd::Scan` off Linux — populate the pairing radar over the seam.
///
/// Not just noise reduction: without this the Windows pairing UI is a dead end.
/// `runScanLoop` in the frontend polls `start_scan` and renders whatever
/// `vortex:scan_result` reports, and the Pair button only exists on a row of
/// that list — so a `Scan` that answers nothing means no row, no button, and no
/// way to reach [`pair_by_scan`] at all. That is exactly what the first Windows
/// run showed: an empty radar and a log full of "UI command has no handler".
///
/// Emits the same three events the BlueZ scan does (`vortex:busy` around it,
/// `vortex:scan_result` per hit, `vortex:scan_done` at the end) because the
/// frontend drives its spinner off them and gives up after 11 s regardless.
///
/// Two honest differences from Linux, both invisible to the user:
///  - One hit at most. `scan_for_peer` resolves on the first Vortex advert it
///    decodes, so it cannot enumerate. One phone is the case that matters, and
///    the SAS comparison is what makes picking the wrong one of two safe.
///  - The row label comes from the advert's Complete Local Name, which most
///    adverts omit — the phone spends its 31-byte budget on the service data.
///    The radar then renders "Android device", same as a nameless hit on Linux.
#[cfg(not(target_os = "linux"))]
pub(crate) fn scan_for_ui(
    app: &tauri::AppHandle,
    central: Arc<dyn BleCentral>,
    active_scan: &mut Option<tokio::task::JoinHandle<()>>,
) {
    use tauri::Emitter;

    // Supersede a still-running scan so handles don't leak, same as the BlueZ
    // path — the frontend re-polls every few seconds and would otherwise stack
    // one radio-holding scan per poll.
    if let Some(prev) = active_scan.take() {
        prev.abort();
    }
    let app = app.clone();
    *active_scan = Some(tokio::spawn(async move {
        let _ = app.emit("vortex:busy", true);
        match central.scan_for_peer(SCAN_MS).await {
            // Pairable only, matching the Linux filter: a trusted-presence
            // advert means that phone is already paired (with us or with
            // another laptop), and offering it as a fresh pair target leads to
            // a handshake failure the user cannot act on.
            Ok(Some(c)) if c.payload.flags.is_pairable() => {
                let hit = crate::ipc::ScanHitDto {
                    addr: c.addr.to_string(),
                    rssi: c.rssi.unwrap_or(0),
                    instance: hex::encode(c.payload.payload_8),
                    name: c.local_name.clone(),
                };
                tracing::info!(
                    addr = %hit.addr, rssi = hit.rssi, instance = %hit.instance,
                    "scan hit",
                );
                let _ = app.emit("vortex:scan_result", hit);
            }
            Ok(Some(c)) => tracing::info!(
                addr = %c.addr,
                "scan saw a Vortex phone that is not in a pairing window; not offered",
            ),
            Ok(None) => tracing::debug!("scan found no Vortex advertisement"),
            Err(e) => tracing::warn!("scan failed: {e}"),
        }
        let _ = app.emit::<Option<()>>("vortex:scan_done", None);
        let _ = app.emit("vortex:busy", false);
    }));
}
