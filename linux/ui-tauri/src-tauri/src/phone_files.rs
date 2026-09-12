//! Browsing the phone's folders from the laptop, and pulling one file out.
//!
//! The counterpart to auto-share: what the phone GENERATES arrives by itself,
//! everything else is asked for. A Download folder is mostly things nobody
//! wants a second copy of, and only the person looking knows which few are the
//! exception — so this shows a list and copies nothing until they pick.
//!
//! Both halves ride the heartbeat that is already up. A listing is a `browse`
//! key in the bulk-sync request and comes back as chunks; a file is queued on
//! the very same pull path an offered capture uses, so the transfer, its
//! progress pill and its save are the proven ones rather than a second
//! implementation.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

/// The folder the UI is waiting to see, and when we last asked for it.
struct Pending {
    at: String,
    asked_at: Option<Instant>,
}

fn pending() -> &'static Mutex<Option<Pending>> {
    static P: Mutex<Option<Pending>> = Mutex::new(None);
    &P
}

/// How long to wait before asking again for a folder that never answered.
///
/// The request rides a heartbeat round, and a round can die between the
/// request and the reply. Consuming the request on send would leave the UI
/// spinning for ever on an unlucky tick; asking every round would put a
/// listing on the wire twelve seconds after the user stopped caring. Keeping
/// it and re-asking on a timer is self-healing without being chatty.
const RETRY_AFTER: Duration = Duration::from_secs(10);

/// Ask the phone for a folder. `at` is a document URI from a previous listing,
/// or empty for the folders the user has granted.
#[tauri::command]
pub fn browse_phone(at: String) {
    if let Ok(mut g) = pending().lock() {
        *g = Some(Pending { at, asked_at: None });
    }
    if let Some(nudge) = crate::SYNC_NUDGE.get() {
        nudge.notify_one();
    }
}

/// Stop waiting on a listing — the user navigated away or closed the page.
#[tauri::command]
pub fn stop_browsing_phone() {
    if let Ok(mut g) = pending().lock() {
        *g = None;
    }
}

/// The folder to put in this round's bulk-sync request, if one is due.
pub(crate) fn browse_request() -> Option<String> {
    let mut g = pending().lock().ok()?;
    let p = g.as_mut()?;
    let due = match p.asked_at {
        None => true,
        Some(t) => t.elapsed() >= RETRY_AFTER,
    };
    if !due {
        return None;
    }
    p.asked_at = Some(Instant::now());
    Some(p.at.clone())
}

/// A listing arrived. Clears the wait when it answers what we asked for, and
/// hands it to the UI either way — a listing we did not ask for is still a
/// true answer about a folder, and dropping it would waste a round-trip.
pub(crate) fn deliver(app: &AppHandle, json: &[u8]) {
    let Some(listing) = vortex_l3_daemon::core::phone_files::PhoneListing::parse(json) else {
        tracing::warn!("phone listing was not parseable JSON; dropped");
        return;
    };
    if let Ok(mut g) = pending().lock() {
        if g.as_ref().is_some_and(|p| p.at == listing.at) {
            *g = None;
        }
    }
    tracing::info!(
        at = %listing.at,
        entries = listing.entries.len(),
        truncated = listing.truncated,
        "phone folder listed"
    );
    let _ = app.emit("vortex:phone_files", listing);
}

/// The `kind` a browsed file is queued under.
///
/// Not a capture — `Offer::subdir` does not know it, so it lands in the
/// download folder rather than the `Phone/` picture tree, which is right for
/// something the user went and picked. But it IS worth telling them about when
/// it lands: they asked for it and then went back to what they were doing, and
/// a pill that fades is the only thing that would otherwise have said where it
/// went. A share pushed from the phone stays silent, as it did.
pub(crate) const FETCHED_KIND: &str = "fetched";

/// Pull one browsed file onto this laptop.
///
/// Queued exactly like an accepted offer, under [`FETCHED_KIND`].
#[tauri::command]
pub fn fetch_phone_file(id: String, name: String, mime: String, bytes: u64) {
    if id.is_empty() {
        return;
    }
    let transfer = crate::transfers::start(&name, bytes, None);
    if let Some(q) = crate::PENDING_FILE_OFFERS.get() {
        if let Ok(mut g) = q.lock() {
            g.push_back((id, name.clone(), mime, transfer, FETCHED_KIND.to_string()));
        }
    }
    crate::lan::note_queue_progress();
    tracing::info!(%name, bytes, "phone file requested → LAN pull nudged");
    if let Some(nudge) = crate::SYNC_NUDGE.get() {
        nudge.notify_one();
    }
}
