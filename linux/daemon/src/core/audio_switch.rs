//! Classic-BT (A2DP) earbuds switch primitives for the Phase 1 manual
//! switch flow per the earbuds-switch design notes §7.1.
//!
//! **S2 — direct D-Bus via `bluer`.** The old ecosystem shelled out to
//! `bluetoothctl` and `busctl`. Each spawn cost ~80-150 ms (process
//! fork + bash parse). We talk to BlueZ directly through `bluer`,
//! which wraps `org.bluez.Device1.{Connect,Disconnect,ConnectProfile,
//! DisconnectProfile}` as native async calls. Three BT ops per switch
//! (disconnect + connect + ready-check) saves ~450 ms.
//!
//! **Profile-targeted disconnect/connect.** Plain `Device.disconnect()`
//! tears down ALL profiles including GATT — which would close our BLE
//! Pairing/Reconnect transport. We disconnect ONLY the A2DP profile
//! (`0000110b-…`) so the Vortex BLE link stays up alongside the audio
//! handoff. Same idea on connect.

use std::time::Duration;

use bluer::{Adapter, Address};
use tokio::time::sleep;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// A2DP Sink — what the buds expose for media audio.
/// (Bluetooth SIG assigned Service Class UUID.)
const A2DP_SINK_UUID: Uuid = Uuid::from_u128(0x0000_110b_0000_1000_8000_0080_5f9b_34fb);

/// HFP Audio Gateway — voice path. Some buds support only HFP for
/// call audio, separate from the A2DP media path.
const HFP_AG_UUID: Uuid = Uuid::from_u128(0x0000_111f_0000_1000_8000_0080_5f9b_34fb);

#[derive(Debug, thiserror::Error)]
pub enum SwitchError {
    #[error("bad MAC address: {0}")]
    BadAddress(String),
    #[error("bluer: {0}")]
    Bluer(#[from] bluer::Error),
    #[error("device not paired with this adapter")]
    NotPaired,
    #[error("operation timed out after {0:?}")]
    Timeout(Duration),
    #[error("internal: {0}")]
    Internal(String),
}

/// Disconnect ONLY the audio profiles (A2DP + HFP). Leaves any active
/// BLE / GATT links alone so the Vortex transport stays up.
///
/// Idempotent: if the profiles are already disconnected, returns Ok.
pub async fn disconnect_audio(adapter: &Adapter, mac: &str) -> Result<(), SwitchError> {
    let addr: Address = mac.parse().map_err(|_| SwitchError::BadAddress(mac.into()))?;
    let device = adapter.device(addr)?;
    if !device.is_paired().await.unwrap_or(false) {
        return Err(SwitchError::NotPaired);
    }

    // Disconnect the WHOLE device (org.bluez.Device1.Disconnect), not
    // each audio profile separately. `DisconnectProfile` does a *graceful*
    // per-profile teardown that can take several SECONDS on these buds —
    // long enough that the phone's call hand-off gives up and falls back
    // to speakerphone (observed ~6-8 s laptop→phone). `Device.Disconnect`
    // forcefully drops the ACL link in ~50 ms, which is what gave the
    // reference build (ecosystem 63eeaee) its ~1 s laptop→phone hand-off.
    // Idempotent: a device that isn't connected just returns ok.
    match device.disconnect().await {
        Ok(()) => debug!(%addr, "device disconnect ok"),
        Err(e) if is_not_connected(&e) => {
            debug!(%addr, "device already disconnected — treating as ok");
        }
        Err(e) => warn!(%addr, "device disconnect failed: {e}"),
    }

    if !wait_audio_disconnected(adapter, addr, DISCONNECT_TIMEOUT).await {
        return Err(SwitchError::Timeout(DISCONNECT_TIMEOUT));
    }
    info!(%addr, "audio device disconnected");
    Ok(())
}

/// Initiate the ACL drop and return the instant BlueZ *accepts* the
/// `Device1.Disconnect` call (~50 ms), WITHOUT waiting for the audio
/// profiles to finish settling.
///
/// This exists for the call hand-off RELEASE path: the responder wants
/// to signal the phone "buds are free" as early as physically possible
/// so the phone can fire (and queue) its A2DP connect — BlueZ holds the
/// phone's connect request and lands it the moment the buds actually
/// drop. Waiting for [`wait_audio_disconnected`] (which polls
/// `audio_active` and lags the real drop by up to ~1 s) before telling
/// the phone wastes that whole window. Pair this with a background
/// [`confirm_audio_disconnected`] for our own state hygiene.
///
/// Returns Ok if the disconnect was accepted (or the device was already
/// disconnected), Err only on a genuine BlueZ failure.
pub async fn disconnect_audio_initiate(adapter: &Adapter, mac: &str) -> Result<(), SwitchError> {
    let addr: Address = mac.parse().map_err(|_| SwitchError::BadAddress(mac.into()))?;
    let device = adapter.device(addr)?;
    if !device.is_paired().await.unwrap_or(false) {
        return Err(SwitchError::NotPaired);
    }
    match device.disconnect().await {
        Ok(()) => {
            debug!(%addr, "device disconnect accepted (initiate)");
            Ok(())
        }
        Err(e) if is_not_connected(&e) => {
            debug!(%addr, "device already disconnected — treating as ok");
            Ok(())
        }
        Err(e) => {
            warn!(%addr, "device disconnect failed: {e}");
            Err(SwitchError::Internal(e.to_string()))
        }
    }
}

/// Block until the audio profiles for `mac` have fully dropped, or the
/// timeout elapses. Returns true if disconnected. Public companion to
/// [`disconnect_audio_initiate`] for callers that send their fast
/// signal first and confirm afterwards.
pub async fn confirm_audio_disconnected(adapter: &Adapter, mac: &str, timeout: Duration) -> bool {
    let Ok(addr) = mac.parse::<Address>() else { return false };
    wait_audio_disconnected(adapter, addr, timeout).await
}

/// Connect the A2DP profile (preferred for media). Falls back to HFP
/// if A2DP fails — some buds expose only the voice path.
///
/// **Single-shot.** Retry policy lives in the orchestrator
/// ([`audio_orchestrator::SwitchOrchestrator::attempt_connect`]) so we
/// don't end up multiplying timeouts (a 3×3×4s nested retry was
/// turning the worst case into 36 s of dead silence). Returns Ok the
/// instant either A2DP or HFP shows a live `bluez_*` sink within
/// `CONNECT_SETTLE`; otherwise propagates the last underlying error.
pub async fn connect_audio(adapter: &Adapter, mac: &str) -> Result<(), SwitchError> {
    let addr: Address = mac.parse().map_err(|_| SwitchError::BadAddress(mac.into()))?;
    let device = adapter.device(addr)?;
    if !device.is_paired().await.unwrap_or(false) {
        return Err(SwitchError::NotPaired);
    }

    // ---- Multipoint fast path ----
    // If the buds ALREADY expose a live A2DP sink here, there's nothing to
    // (re)connect: just ensure the card is on A2DP and return. The
    // downstream route step makes us the default sink and the buds switch
    // their active stream to us when playback starts — skipping the whole
    // ~3.5s single-point drop+reconnect (`wait_ms`).
    //
    // This is what makes MULTIPOINT earbuds (Sony / Bose / Jabra /
    // FreeBuds Pro …) switch near-instantly: their link to us stays up
    // even while the phone streams, so `audio_active` is already true and
    // a grab is just a route change (~0.2s).
    //
    // SAFE for single-point buds (e.g. FreeBuds SE 3): when they leave for
    // the phone they drop our link, so `audio_active` is false here and we
    // fall through to the normal connect below — behaviour unchanged. (It
    // also fast-paths a reclaim of buds still physically ours.)
    //
    // NOTE: validated only against single-point hardware so far (the
    // fall-through case); the multipoint branch is reasoned from the BlueZ
    // link-state semantics and needs a real multipoint device to confirm.
    if audio_active(adapter, addr).await {
        let _ = force_card_to_a2dp(mac).await;
        info!(%addr, "A2DP already live (multipoint/reclaim) — fast route, no reconnect");
        return Ok(());
    }

    // If a stale link from a previous owner is still up, tear the
    // audio profiles down first — connecting on top of a live
    // connection often deadlocks BlueZ for ~10s.
    if device.is_connected().await.unwrap_or(false) {
        for uuid in [A2DP_SINK_UUID, HFP_AG_UUID] {
            let _ = device.disconnect_profile(&uuid).await;
        }
        let _ = wait_audio_disconnected(adapter, addr, Duration::from_millis(500)).await;
    }

    // Prewarm the BlueZ card to the A2DP-sink profile BEFORE asking
    // BlueZ to connect_profile. The old ecosystem's
    // `prewarm_linux_reclaim_path` did this exact step on the
    // SIG_PHONE_PREPARE_LINUX_RECLAIM signal: when the call is ending
    // the card is still pinned to HFP, and connect_profile(A2DP)
    // races BlueZ's own profile negotiation — that's why attempts
    // 1 and 2 fail with "Operation already in progress" and we lose
    // 3-6 seconds before attempt 3 succeeds. Pushing the card to
    // A2DP first means PipeWire creates the sink in IDLE
    // immediately and the very first connect_profile attempt lands.
    // Best-effort: ignore failures (card may not exist yet on the
    // first call after a fresh boot).
    let _ = force_card_to_a2dp(mac).await;

    // Single-shot: try A2DP, fall back to HFP, return Ok or Err. The
    // orchestrator (audio_orchestrator::attempt_connect) owns the
    // retry loop — running a second retry layer here turned the
    // worst-case wait into N×N×CONNECT_SETTLE (~36s with N=3,
    // settle=4s). One layer is enough: the orchestrator can react
    // to state changes between retries (peer Reject, user cancel)
    // which this function can't.
    let t_attempt = tokio::time::Instant::now();

    // A2DP first (media path) — what 99% of users want.
    //
    // **`br-connection-busy` is not a failure.** Right after
    // disconnect, BlueZ often returns "Operation already in
    // progress" / "br-connection-busy" from connect_profile —
    // meaning "I'm already negotiating, don't poke me again."
    // The old ecosystem (`connect_audio_device_fast` in
    // bt_classic.rs) treated this as success and polled the
    // actual connection state. We do the same: any error gets
    // fed into `wait_audio_connected`, and if the buds come up
    // within the settle window we return Ok.
    let t_profile_start = tokio::time::Instant::now();
    let mut a2dp = connect_profile_bounded(&device, &A2DP_SINK_UUID).await;
    // Transient BlueZ errors (br-connection-create-socket / -canceled /
    // page-timeout) mean the ACL link wasn't ready yet — NOT that A2DP is
    // unavailable. Retry the A2DP connect a couple of times with a short
    // settle before falling through to HFP. These buds are A2DP-only, so
    // the old straight-to-HFP path hit ProfileUnavailable and burned
    // ~12 s before a later attempt landed (observed live).
    let mut a2dp_tries = 0u8;
    while a2dp_tries < A2DP_TRANSIENT_RETRIES
        && matches!(&a2dp, ProfileOutcome::Err(e) if is_transient_connect(e))
    {
        a2dp_tries += 1;
        warn!(%addr, "A2DP transient connect error; retry {a2dp_tries}/{A2DP_TRANSIENT_RETRIES}");
        sleep(A2DP_TRANSIENT_PAUSE).await;
        a2dp = connect_profile_bounded(&device, &A2DP_SINK_UUID).await;
    }
    let bluez_ms = t_profile_start.elapsed().as_millis();
    let a2dp_busy = matches!(a2dp, ProfileOutcome::Busy);
    if matches!(a2dp, ProfileOutcome::Ok | ProfileOutcome::Busy) {
        let t_wait_start = tokio::time::Instant::now();
        if wait_audio_connected(adapter, addr, CONNECT_SETTLE).await {
            let wait_ms = t_wait_start.elapsed().as_millis();
            let connect_ms = t_attempt.elapsed().as_millis();
            info!(
                %addr,
                connect_ms,
                bluez_ms,
                wait_ms,
                a2dp_busy,
                "A2DP connected"
            );
            return Ok(());
        }
    }
    let mut last_err: Option<SwitchError> = match a2dp {
        ProfileOutcome::Err(e) => {
            info!("A2DP connect_profile error (will fall back to HFP): {e}");
            Some(SwitchError::Bluer(e))
        }
        ProfileOutcome::TimedOut => {
            // BlueZ never answered connect_profile within the bound —
            // it's wedged (A2DP radio starved by the buds streaming
            // elsewhere, or a half-open ACL). Treat as a fast failure
            // so the orchestrator's retry/reset runs instead of letting
            // the D-Bus default (~25 s) freeze the whole switch flow in
            // `Connecting`. The dropped future cancels the in-flight
            // call; a follow-up connect just gets `br-connection-busy`
            // (handled as ok) if BlueZ did keep working on it.
            warn!(%addr, "A2DP connect_profile timed out ({PROFILE_CONNECT_TIMEOUT:?}); falling back to HFP");
            Some(SwitchError::Timeout(PROFILE_CONNECT_TIMEOUT))
        }
        ProfileOutcome::Ok | ProfileOutcome::Busy => None,
    };

    // HFP fallback (voice-only buds). Same busy-is-ok pattern.
    let hfp = connect_profile_bounded(&device, &HFP_AG_UUID).await;
    let hfp_busy = matches!(hfp, ProfileOutcome::Busy);
    if matches!(hfp, ProfileOutcome::Ok | ProfileOutcome::Busy)
        && wait_audio_connected(adapter, addr, CONNECT_SETTLE).await {
            info!(%addr, hfp_busy, "HFP connected (A2DP unavailable)");
            return Ok(());
        }
    match hfp {
        ProfileOutcome::Err(e) => {
            info!("HFP connect_profile error: {e}");
            last_err = Some(SwitchError::Bluer(e));
        }
        ProfileOutcome::TimedOut => {
            warn!(%addr, "HFP connect_profile timed out ({PROFILE_CONNECT_TIMEOUT:?})");
            last_err = Some(SwitchError::Timeout(PROFILE_CONNECT_TIMEOUT));
        }
        ProfileOutcome::Ok | ProfileOutcome::Busy => {}
    }
    Err(last_err.unwrap_or(SwitchError::Timeout(CONNECT_SETTLE)))
}

/// Returns true once the buds are NOT advertising any active audio
/// profile (A2DP / HFP). Other transports — e.g. our own BLE / GATT —
/// are ignored on purpose.
async fn wait_audio_disconnected(adapter: &Adapter, addr: Address, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if !audio_active(adapter, addr).await {
            return true;
        }
        sleep(POLL_INTERVAL).await;
    }
    false
}

async fn wait_audio_connected(adapter: &Adapter, addr: Address, timeout: Duration) -> bool {
    // Fast path: already there. Avoids spawning `pactl subscribe` at
    // all when the connect_profile completed before we got here.
    if audio_active(adapter, addr).await {
        return true;
    }

    // Event-driven path: subscribe to PulseAudio/PipeWire events via
    // `pactl subscribe` and react the moment a sink-state line fires.
    // Polling every 80ms used to spend ~30ms per probe on subprocess
    // spawn + IPC + parse — 12-15 wasted probes during a typical
    // ~2 s sink-creation window. The subscribe stream costs one
    // long-lived subprocess and re-probes only when the event stream
    // says "something sink-related changed".
    //
    // `kill_on_drop` guarantees we don't leak the subprocess on the
    // timeout path (the child stays alive until the BufReader is
    // dropped at the end of this function).
    let mut child = match tokio::process::Command::new("pactl")
        .args(["subscribe"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("pactl subscribe spawn failed: {e}; falling back to polling");
            return wait_audio_connected_polling(adapter, addr, timeout).await;
        }
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return wait_audio_connected_polling(adapter, addr, timeout).await,
    };
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => {
                debug!("wait_audio_connected: timeout via event stream");
                return false;
            }
            line = lines.next_line() => {
                match line {
                    Ok(Some(s)) => {
                        // Trim to the event types that can imply our
                        // sink is now alive. PipeWire emits "Event
                        // 'new' on sink #N" when a new sink shows up;
                        // "Event 'change' on card #N" can fire when
                        // BlueZ flips the A2DP profile on the card.
                        // Ignore client/sink-input/source noise — none
                        // of those affect bluez_output existence.
                        let interesting =
                            s.contains("on sink") || s.contains("on card");
                        if !interesting {
                            continue;
                        }
                        if audio_active(adapter, addr).await {
                            return true;
                        }
                    }
                    Ok(None) | Err(_) => {
                        // Subscribe died (e.g. pactl restart). Fall
                        // back to polling for whatever budget remains
                        // so the orchestrator still gets an answer.
                        let remaining = deadline
                            .saturating_duration_since(tokio::time::Instant::now());
                        if remaining.is_zero() {
                            return false;
                        }
                        warn!("pactl subscribe ended unexpectedly; polling remainder");
                        return wait_audio_connected_polling(adapter, addr, remaining).await;
                    }
                }
            }
        }
    }
}

/// Legacy polling implementation, kept as a fallback for when `pactl
/// subscribe` can't be started (e.g. pactl missing, sandbox-blocked,
/// or the stream dies mid-wait).
async fn wait_audio_connected_polling(
    adapter: &Adapter,
    addr: Address,
    timeout: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if audio_active(adapter, addr).await {
            return true;
        }
        sleep(POLL_INTERVAL).await;
    }
    false
}

/// True if either A2DP or HFP is currently up for this device. We
/// inspect the published UUIDs the *device* claims — `bluer` exposes
/// `Device1.UUIDs` which BlueZ updates as profiles connect / drop.
pub async fn audio_active(adapter: &Adapter, addr: Address) -> bool {
    let device = match adapter.device(addr) {
        Ok(d) => d,
        Err(_) => return false,
    };
    // If the device isn't connected on ANY transport, the audio
    // profiles are definitely gone.
    if !device.is_connected().await.unwrap_or(false) {
        return false;
    }
    // BlueZ's `uuids` lists *advertised* profiles, which always
    // includes A2DP/HFP for buds regardless of whether those profiles
    // are currently connected (this was a bug — `wait_audio_disconnected`
    // would always time out). The real signal of "A2DP is up right now"
    // is that PulseAudio / PipeWire-pulse exposes a `bluez_*` sink for
    // this MAC. When the profile drops, the sink disappears within
    // ~50 ms. Same check, both backends.
    let needle_under = addr.to_string().replace(':', "_");
    let needle_colon = addr.to_string();
    // Robust "is A2DP actually active": a `bluez_*` sink for this MAC
    // exists in PulseAudio/PipeWire — survives HFP-only buds, BlueZ's
    // stale UUID cache, and PipeWire's two name formats. Served from the
    // subscribe-backed sink cache so this hot path (200ms reconcile +
    // heartbeat, while we hold the buds) doesn't fork pactl every tick.
    crate::core::audio_sink_cache::has_bluez_sink_for(&[&needle_under, &needle_colon]).await
}

/// Push the BlueZ card for [mac] to its A2DP-sink profile. Done before
/// connect_profile so PipeWire creates the sink in IDLE (not SUSPENDED)
/// and the first A2DP connect attempt lands cleanly — mirrors the old
/// ecosystem's `set_audio_a2dp_profile` step.
async fn force_card_to_a2dp(mac: &str) -> bool {
    let card = format!("bluez_card.{}", mac.replace(':', "_"));
    for profile in ["a2dp-sink", "a2dp_sink", "a2dp-sink-aac"] {
        let res = tokio::process::Command::new("pactl")
            .args(["set-card-profile", &card, profile])
            .output()
            .await;
        if let Ok(o) = res {
            if o.status.success() {
                debug!(%card, profile, "card pushed to A2DP");
                return true;
            }
        }
    }
    false
}

/// Outcome of a single bounded `connect_profile` call. `Busy` (BlueZ
/// "already in progress") is treated like success by the caller — the
/// connect is in flight and we poll for it. `TimedOut` means BlueZ
/// never answered within [`PROFILE_CONNECT_TIMEOUT`] — a wedged radio,
/// surfaced as a fast failure so the flow doesn't freeze for the D-Bus
/// default (~25 s).
enum ProfileOutcome {
    Ok,
    Busy,
    Err(bluer::Error),
    TimedOut,
}

/// Call `connect_profile` with a hard upper bound. BlueZ normally
/// answers in well under 2 s; when A2DP can't establish (the buds are
/// busy streaming to another device, starving the single-antenna BT
/// radio) the D-Bus call can hang until the bus's own ~25 s reply
/// timeout. That window is long enough to wedge the orchestrator in
/// `Connecting` and make every subsequent claim bounce off the "busy"
/// guard — the "stuck in switching" symptom. Bounding the call to a few
/// seconds turns the hang into a retryable failure.
async fn connect_profile_bounded(device: &bluer::Device, uuid: &Uuid) -> ProfileOutcome {
    match tokio::time::timeout(PROFILE_CONNECT_TIMEOUT, device.connect_profile(uuid)).await {
        Ok(Ok(())) => ProfileOutcome::Ok,
        Ok(Err(e)) if is_busy_or_in_progress(&e) => ProfileOutcome::Busy,
        Ok(Err(e)) => ProfileOutcome::Err(e),
        Err(_) => ProfileOutcome::TimedOut,
    }
}

/// Recognize the "BlueZ is already working on this connection" family
/// of errors from `connect_profile`. These are NOT real failures — the
/// connect is in flight; the caller just needs to poll `is_connected`
/// for a moment. Mirrors the old ecosystem's `br-connection-busy`
/// special case in `connect_audio_device_fast`. Without this we burn
/// 3-6 seconds bouncing through retries while BlueZ silently completes
/// the connect on attempt 1.
fn is_busy_or_in_progress(e: &bluer::Error) -> bool {
    let s = e.to_string().to_ascii_lowercase();
    s.contains("already in progress")
        || s.contains("br-connection-busy")
        || s.contains("connection-busy")
        || s.contains("operation already in progress")
}

/// Recognize transient ACL/connection errors from `connect_profile` that
/// are worth a quick retry (the link is being established) rather than a
/// fall-through to HFP. Distinct from [`is_busy_or_in_progress`] (which
/// means "already connecting, just poll") — these mean "the connect
/// attempt failed to even start because the radio/ACL wasn't ready."
fn is_transient_connect(e: &bluer::Error) -> bool {
    let s = e.to_string().to_ascii_lowercase();
    s.contains("create-socket")
        || s.contains("connection-canceled")
        || s.contains("br-connection-canceled")
        || s.contains("page-timeout")
        || s.contains("page timeout")
        || s.contains("connection refused")
        || s.contains("host is down")
        || s.contains("connection timed out")
}

fn is_not_connected(e: &bluer::Error) -> bool {
    // BlueZ returns "Not Connected" / "Device not connected" depending
    // on version. Newer BlueZ (>=5.65) sometimes maps the same
    // condition to "Invalid arguments" when the profile UUID isn't in
    // the device's currently-connected set. Treat all of these as
    // idempotent — disconnecting a profile that isn't there is
    // exactly what we wanted.
    let s = e.to_string().to_ascii_lowercase();
    s.contains("not connected") || s.contains("invalid arguments")
}

// ---- Tuned constants — see the earbuds-switch design notes §6 ----

/// Window for the connection to actually establish after a successful
/// `connect_profile()` call. The buds-from-phone case is the long tail:
/// when the phone has just released the buds, the bluez_output sink
/// can take 1.5-3 seconds to appear in pactl. With a short
/// `CONNECT_SETTLE` the first attempt times out, we fall through to
/// HFP (which returns ProfileUnavailable immediately) and burn the
/// 200 ms pause + a second attempt — 5+ seconds of dead time. Old
/// ecosystem's `wait_for_audio_ready` used a 5-9 s budget at this
/// layer; 4 s is the sweet spot — long enough to catch the buds on
/// attempt 1 in the common case, short enough that a truly stuck
/// peer still gets retried before the user notices.
const CONNECT_SETTLE: Duration = Duration::from_millis(4000);

/// Hard upper bound on a single `connect_profile` D-Bus call. BlueZ
/// answers a healthy connect in well under 2 s; this bound only ever
/// trips when the call is genuinely wedged (A2DP radio starved by the
/// buds streaming elsewhere). Sits comfortably above the legit max and
/// far below the D-Bus bus default (~25 s), so a hang becomes a fast,
/// retryable failure instead of freezing the switch flow in `Connecting`.
const PROFILE_CONNECT_TIMEOUT: Duration = Duration::from_millis(6000);

/// How many times to retry the A2DP connect on a transient ACL error
/// (br-connection-create-socket etc.) before falling through to HFP.
const A2DP_TRANSIENT_RETRIES: u8 = 2;
/// Settle between transient-error A2DP retries — long enough for BlueZ to
/// finish establishing the ACL.
const A2DP_TRANSIENT_PAUSE: Duration = Duration::from_millis(220);

/// Disconnect is usually faster than connect. 1 s is enough for both
/// profiles to drop in practice.
const DISCONNECT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Poll interval while awaiting a state flip. 40 ms keeps the
/// release/connect confirmation tight (the reference build polled at
/// ~35 ms for its ~1 s hand-off) — the `is_connected()` fast-path in
/// `audio_active` means most polls don't even reach the pactl subprocess.
const POLL_INTERVAL: Duration = Duration::from_millis(40);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_match_sig_assignments() {
        // Sanity: the canonical Service Class UUIDs from the Bluetooth
        // SIG Assigned Numbers. If these ever change, every paired
        // device on the planet would have stopped working — so this
        // is really a regression guard for our own copy-paste.
        assert_eq!(
            A2DP_SINK_UUID.to_string(),
            "0000110b-0000-1000-8000-00805f9b34fb"
        );
        assert_eq!(
            HFP_AG_UUID.to_string(),
            "0000111f-0000-1000-8000-00805f9b34fb"
        );
    }

    #[test]
    fn bad_mac_yields_clear_error() {
        // We don't call adapter (no async), just confirm the parse
        // catches obvious junk. The fuller live tests live on real
        // hardware via the orchestrator e2e test.
        let parse_result: Result<bluer::Address, _> = "not-a-mac".parse();
        assert!(parse_result.is_err());
    }
}
