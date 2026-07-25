//! File-transfer indicator wiring — split out of `worker::run_worker` to keep
//! that function focused on the BLE/LAN lifecycle + command loop. Wires the two
//! progress pills (incoming pull + outgoing batch push) and the receive-consent
//! action router. Everything here talks to process-global state
//! (`PENDING_FILE_OFFERS`, the daemon progress hooks, the `transfers*` pills),
//! so the only handle it needs is the shared live-activity channel.

use tokio::sync::mpsc::UnboundedSender;
use vortex_l3_daemon::core::live_activity::LiveActivity;

/// Initialise both transfer pills and install the daemon progress/consent hooks.
/// Call once from `run_worker` after the live-activity consumer is up.
pub(crate) fn wire_transfer_indicators(ble_live_tx: UnboundedSender<LiveActivity>) {
    // Ongoing INCOMING transfer pill (the same live-activity channel as the call pill).
    crate::transfers::init(ble_live_tx.clone());
    // Live file-transfer progress: the daemon's LAN pull reports chunk counts →
    // forward (throttled) to the FRONT queued file's indicator.
    vortex_l3_daemon::core::file_progress::set_hook(Box::new(move |rc, tc| {
        // ~10 updates per file keeps the pill smooth without thrashing the
        // tray-fallback's icon/menu rebuild on every chunk.
        let step = (tc / 10).max(1);
        if rc % step != 0 && rc != tc {
            return;
        }
        let id = crate::PENDING_FILE_OFFERS
            .get()
            .and_then(|m| m.lock().ok().and_then(|g| g.front().map(|e| e.3)));
        if let Some(id) = id {
            crate::transfers::set_progress_chunks(id, rc, tc);
        }
    }));

    // OUTGOING (laptop→phone) batch push → aggregate "Sharing/Sending" pill,
    // mirror of the receive pill above. The daemon reports Start/Accepted/
    // Declined/Progress/Done/Fail. Progress fires per chunk, so throttle to
    // ~1%-granularity (last reported pct held in an atomic) to keep the GNOME
    // pill / tray fallback from thrashing on big transfers.
    crate::transfers_out::init(ble_live_tx);
    let last_pct = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(-1));
    vortex_l3_daemon::core::outgoing_share::set_progress_hook(Box::new(move |ev| {
        use std::sync::atomic::Ordering;
        use vortex_l3_daemon::core::outgoing_share::OutProgress;
        match ev {
            OutProgress::Start { label, total, .. } => {
                last_pct.store(-1, Ordering::Relaxed);
                crate::transfers_out::start(&label, total);
            }
            OutProgress::Accepted => crate::transfers_out::accepted(),
            OutProgress::Declined => crate::transfers_out::declined(),
            OutProgress::Progress { sent, total } => {
                let pct = if total > 0 { (sent * 100 / total) as i64 } else { 0 };
                if pct != last_pct.load(Ordering::Relaxed) || sent == total {
                    last_pct.store(pct, Ordering::Relaxed);
                    crate::transfers_out::set_progress(sent, total);
                }
            }
            OutProgress::Done => crate::transfers_out::complete(),
            OutProgress::Fail => crate::transfers_out::fail(),
        }
    }));

    // Phone→laptop receive consent: route fc:accept/fc:decline banner clicks
    // back to the waiting offer-consumer (instant-share style Accept/Decline).
    tokio::spawn(crate::file_consent::watch());
}
