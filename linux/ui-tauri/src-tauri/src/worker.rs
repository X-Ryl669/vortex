//! Worker thread: owns the BLE adapter, identity, peer store, and drives
//! the UiCmd channel loop. Emits Tauri events for every state change so
//! the Vue layer can render reactively. Split out of lib.rs.

use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, State};

use vortex_l3_daemon::core::identity::Platform;
use vortex_l3_daemon::core::storage::peers::{PeerStore, SecretServicePeerStore};
use vortex_l3_daemon::core::storage::secret_service::SecretServiceIdentityStore;
use vortex_l3_daemon::core::storage::{load_or_generate, IdentityStore, InMemoryIdentityStore};

use crate::ble::run_ble_persistent_loop;
use crate::call::spawn_consumer as spawn_call_consumer;
use crate::call_log::spawn_consumer as spawn_call_log_consumer;
use crate::contacts::spawn_consumer as spawn_contacts_consumer;
use crate::ipc::{emit_peers, CmdChannel, IdentityInfo, TrustedPeerDto, UiCmd};
use crate::lan::{self, load_last_peer_ip, try_lan_reconnect};
use crate::live_activity::spawn_consumer as spawn_live_consumer;
use crate::sms::{self, spawn_consumer as spawn_sms_consumer};
use crate::{
    cmd_earbuds, cmd_pairing, earbuds, notifications, worker_ctx, BLE_RETRY_NUDGE, CALL_MIRROR_TX,
    CALL_WRITER, SYNC_NUDGE,
};

#[tauri::command]
pub fn start_scan(state: State<'_, CmdChannel>) -> Result<(), String> {
    state.0.send(UiCmd::Scan).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn refresh_state(state: State<'_, CmdChannel>) -> Result<(), String> {
    state.0.send(UiCmd::RefreshState).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn start_screen_mirror(
    state: State<'_, CmdChannel>,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
) -> Result<(), String> {
    state
        .0
        .send(UiCmd::StartMirror { width, height, fps, bitrate })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn stop_screen_mirror(state: State<'_, CmdChannel>) -> Result<(), String> {
    state.0.send(UiCmd::StopMirror).map_err(|e| e.to_string())
}

// --------------------------------------------------------------------------
// Worker — owns the BLE adapter, identity, peer store, and drives the
// channel loop. Emits Tauri events for every state change so the Vue
// layer can render reactively.
// --------------------------------------------------------------------------

pub(crate) fn run_worker(app: AppHandle, cmd_rx: Receiver<UiCmd>) {
    // Prime the LAN fast-path from disk so the first heartbeat after a restart
    // reuses the last-known phone IP instead of guessing the gateway.
    load_last_peer_ip();
    let rt = tokio::runtime::Builder::new_multi_thread()
        // 8 threads instead of 2: sync secret-service D-Bus calls
        // (peer_store.list/save/load_counter) block their executor
        // thread, and right after a re-pair the heartbeat and BLE
        // persistent loops can both hit them simultaneously. With
        // only 2 worker threads, that wedges the whole runtime —
        // timers stop firing and the whole UI goes silent. 8 leaves
        // plenty of headroom; spawn_blocking around the sync calls
        // would be the cleaner long-term fix.
        .worker_threads(8)
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async move {
        // Identity store: Secret Service is mandatory per the V1
        // security baseline ("if secure storage is unavailable, V1
        // MUST stop and show an error"). We surface a vortex:fatal
        // event so the UI can render a banner instead of silently
        // downgrading to an in-memory identity that would persist
        // nothing across restarts.
        let id_store: Box<dyn IdentityStore> = match SecretServiceIdentityStore::new() {
            Ok(s) => Box::new(s),
            Err(err) => {
                tracing::error!("FATAL: secret-service unavailable ({err}); cannot start");
                let _ = app.emit(
                    "vortex:fatal",
                    format!("Secure storage unavailable: {err}. Unlock your keyring and restart Vortex."),
                );
                // Honour the long-standing dev escape hatch for hermetic
                // test environments (CI without a session keyring).
                if std::env::var("VORTEX_INSECURE").as_deref() == Ok("1") {
                    tracing::warn!("VORTEX_INSECURE=1 — falling back to in-memory identity (dev only)");
                    Box::new(InMemoryIdentityStore::new())
                } else {
                    return;
                }
            }
        };
        let identity = match load_or_generate(&*id_store, Platform::Linux) {
            Ok(id) => id,
            Err(err) => {
                tracing::error!("FATAL: identity init failed: {err}");
                return;
            }
        };
        let _ = app.emit("vortex:identity", IdentityInfo { ready: true });

        // Peer store.
        let peer_store: Arc<dyn PeerStore> = match SecretServicePeerStore::new() {
            Ok(s) => Arc::new(s),
            Err(err) => {
                tracing::error!("secret-service peer store unavailable: {err}");
                let _ = app.emit::<Vec<TrustedPeerDto>>("vortex:peers", Vec::new());
                return;
            }
        };
        emit_peers(&app, peer_store.clone()).await;
        let _have_trust = !peer_store.list().unwrap_or_default().is_empty();

        // BLE adapter.
        let session = match bluer::Session::new().await {
            Ok(s) => s,
            Err(err) => {
                tracing::error!("BLE session init failed: {err}");
                return;
            }
        };
        let adapter = match session.default_adapter().await {
            Ok(a) => a,
            Err(err) => {
                tracing::error!("BLE adapter init failed: {err}");
                return;
            }
        };
        let _ = adapter.set_powered(true).await;

        // ----- BlueZ pairing agent (BT bond, Just Works) -----
        // We register a NoInputNoOutput agent so `device.pair()` in
        // `do_pair` can complete Just Works bonding without any PIN
        // dialog on this side. Only `request_authorization` is wired up
        // (auto-accept); `request_confirmation` is intentionally left
        // unset — supplying it would push BlueZ into DisplayYesNo
        // capability and trigger numeric-comparison flows. The bond is
        // safe under Just Works because by the time `do_pair` calls
        // `device.pair()` the peer is already authenticated via
        // Noise+SAS (our app-layer MITM defence runs *before* the BT
        // bond — see do_pair). The `AgentHandle` MUST be kept alive for
        // the worker's lifetime; dropping it unregisters the agent and
        // BlueZ falls back to its default (which on a typical desktop
        // session would prompt the user).
        let _agent_handle = match session
            .register_agent(bluer::agent::Agent {
                request_default: false,
                request_authorization: Some(Box::new(|_req| {
                    Box::pin(async move { Ok(()) })
                })),
                ..Default::default()
            })
            .await
        {
            Ok(h) => {
                tracing::info!("BlueZ pairing agent registered (Just Works)");
                Some(h)
            }
            Err(e) => {
                // Non-fatal: pairing still works at the Noise layer; only
                // the BT-level bond is skipped, which means the persistent
                // BLE loop will continue to chase rotating RPAs by scan
                // (current pre-bond behaviour).
                tracing::warn!("BlueZ agent register failed: {e}; bonding disabled this session");
                None
            }
        };

        // ----- Earbuds-switch orchestrator + media follow (Phases 1–3) -----
        // The whole audio/earbuds wiring (orchestrator + race-for-first-
        // success sender, smart-follow watcher, media runtime, resume
        // watcher, switch-state bridge) lives in earbuds::setup_audio.
        let earbuds::AudioSetup {
            session_writers,
            ble_audio_writers,
            switch_orchestrator,
            media_watch,
            media_in_call,
            media_store,
        } = earbuds::setup_audio(&app, &adapter, peer_store.clone()).await;
        // Tracks the most recent call_phase seen from the phone so
        // try_lan_reconnect reacts only on transitions (e.g. null →
        // ringing), not every steady-state heartbeat.
        let last_call_phase: Arc<tokio::sync::Mutex<Option<String>>> =
            Arc::new(tokio::sync::Mutex::new(None));

        // Continuous auto-reconnect / refresh loop. Each pass does a
        // fresh IK + ping/pong + app-state exchange (~150ms locally).
        // The Mutex guards against overlapping reconnects when the
        // user pokes a manual action while we're in the middle of one.
        let auto_lock = Arc::new(tokio::sync::Mutex::new(()));

        // Tracks when any path last completed a LAN reconnect. The
        // mDNS wake-up uses this as a cooldown gate: mdns-sd re-resolves
        // `_vortex._tcp` every couple of seconds (TTL refresh /
        // re-announce) even while we're already connected, and without
        // this guard each resolve fired a fresh full TCP+IK handshake —
        // 13 in ~27 s in one observed run. That storm burned the trust
        // counter and, by hammering libsecret/BlueZ D-Bus over and over,
        // wedged the executor (the very hazard the spawn_blocking note
        // below guards against). The 12 s heartbeat still refreshes the
        // link; mDNS now only pounces when the phone has actually been
        // gone longer than the cooldown.
        let last_reconnect_at: Arc<tokio::sync::Mutex<Option<tokio::time::Instant>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        const MDNS_RECONNECT_COOLDOWN: Duration = Duration::from_secs(10);

        // Event-driven push: a local state change (laptop charging flip or a
        // meaningful battery-level delta) fires this Notify to wake the
        // heartbeat loop immediately instead of waiting out the 12/45 s tick.
        // The periodic tick stays as a liveness floor + safety net for any
        // missed wake. This is the universal "state changed → sync now"
        // primitive — future fields just call `sync_nudge.notify_one()`.
        let sync_nudge = Arc::new(tokio::sync::Notify::new());
        // Publish the nudge so Tauri commands (e.g. the smart-switch toggle)
        // can wake the heartbeat to push the new state immediately.
        let _ = SYNC_NUDGE.set(sync_nudge.clone());
        // Pending phone-shared image token (LAN-pulled via bulk-sync).
        let _ = crate::PENDING_IMAGE_TOKEN.set(std::sync::Mutex::new(None));
        // …and the queue of instant-share files awaiting their LAN pull.
        let _ = crate::PENDING_FILE_OFFERS.set(std::sync::Mutex::new(std::collections::VecDeque::new()));

        // BLE-side twin: the LAN heartbeat fires this on its down→up edge so
        // the BLE presence wait retries the moment the phone shows up on the
        // network (the cross-transport presence hint).
        let ble_retry_nudge = Arc::new(tokio::sync::Notify::new());
        let _ = BLE_RETRY_NUDGE.set(ble_retry_nudge.clone());

        // BLE state-push channel: the persistent BLE listener forwards a
        // peer STATE frame (battery/charging) here as (peer_pub, AppState);
        // a consumer task applies it to the UI instantly — the same Vue
        // event + tray refresh a LAN heartbeat produces, but in ~200 ms over
        // the already-open BLE link instead of a fresh TCP+IK reconnect.
        let ble_state_tx = crate::lan_state::spawn_state_consumer(app.clone());

        // BLE notification-mirror channel: the persistent listener forwards
        // a decoded NotificationMirror here; a consumer pops it as a desktop
        // notification via org.freedesktop.Notifications. Content is not
        // logged (privacy) beyond the app label.
        // BLE live-activity channel: the persistent listener forwards decoded
        // LiveActivity updates here; this consumer drives the top-bar tray
        // "pill" — on Linux `set_title` shows a text label next to the tray
        // icon (libappindicator label), updated in place as the ETA changes.
        // Live-activity style. Content not logged beyond the app label.
        // Call-card actions: the GNOME extension's in-call-pill buttons
        // (Mute/Speaker/End) call CallAction on the live-activity D-Bus
        // interface → this channel → the call consumer → the phone.
        let (call_action_tx, call_action_rx) =
            tokio::sync::mpsc::unbounded_channel::<String>();
        let ble_live_tx = spawn_live_consumer(app.clone(), call_action_tx).await;

        // BLE call-mirror channel: the listener forwards CALL frames here; the
        // consumer drives the laptop's call banner (ringing → Accept/Decline)
        // and in-call pill (caller + live duration → Mute/End). The writer
        // handle carries the user's banner clicks back to the phone.
        let (ble_call_tx, ble_call_writer) =
            spawn_call_consumer(app.clone(), ble_live_tx.clone(), call_action_rx).await;
        // Expose the call-control writer globally so the `dial` command can
        // place a laptop-initiated call over the live BLE link.
        let _ = CALL_WRITER.set(ble_call_writer.clone());

        // BLE contacts-mirror channel: the listener forwards CONTACTS chunks
        // here; the consumer reassembles, caches, and emits to the Contacts page.
        let ble_contacts_tx = spawn_contacts_consumer(app.clone()).await;

        // BLE call-log-mirror channel: same pattern for the Recents page.
        let ble_call_log_tx = spawn_call_log_consumer(app.clone()).await;

        // BLE SMS-mirror channel: same pattern for the Messages page.
        let ble_sms_tx = spawn_sms_consumer(app.clone()).await;
        // BLE on-demand SMS-thread channel: a single conversation's page (the
        // Messages page's infinite scroll), MERGED into the open thread instead
        // of replacing the recent list.
        let ble_sms_thread_tx = sms::spawn_thread_consumer(app.clone()).await;
        // Publish the call-mirror sender so the LAN heartbeat can feed a peer
        // AppState's `call` into the same consumer (additive LAN path).
        let _ = CALL_MIRROR_TX.set(ble_call_tx.clone());

        // BLE browsing-handoff channel: the listener forwards HANDOFF frames
        // here; the consumer opens a shared page (Share) or shows a "continue
        // from phone" pill (live read) the user clicks to open.
        let ble_handoff_tx = crate::handoff::spawn_consumer(app.clone(), ble_live_tx.clone());
        // Publish it so a peer AppState's `handoff` (the LAN backstop) feeds the
        // same consumer alongside the dedicated BLE HANDOFF frame.
        let _ = crate::handoff::HANDOFF_TX.set(ble_handoff_tx.clone());

        // Notes bidirectional sync: a generic sealed-frame writer (filled on
        // connect) + the raw-frame channel the listener forwards NOTES_SYNC
        // into. All merge/protocol logic lives in notes.rs.
        let ble_sealed_writer: Arc<tokio::sync::Mutex<Option<crate::SealedWriter>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        let ble_notes_tx = crate::notes::spawn_sync(app.clone(), ble_sealed_writer.clone());
        crate::notes::spawn_reminders(); // desktop due-date reminders

        // BLE app-icon channel: the listener forwards ICON chunks here; a
        // consumer reassembles each app's PNG and caches it to disk so
        // mirrored notifications can show the real app logo.
        let ble_icon_tx = notifications::spawn_icon_consumer();

        // Phone→laptop notification mirror + laptop→phone capture +
        // dismiss/action sync — see notifications::spawn_subsystem.
        let (ble_notif_tx, ble_notif_writer) = notifications::spawn_subsystem(app.clone());

        // Clipboard sync (P2): phone→laptop receive (set clipboard + history)
        // + laptop→phone send (the watcher queues; this drains via the writer).
        let (ble_clipboard_tx, ble_clipboard_writer, ble_clipboard_image_tx, ble_clipboard_image_writer) =
            crate::clipboard_sync::spawn_clipboard_sync(app.clone());
        // Phone image/file-offer → stash + nudge the LAN heartbeat to pull it.
        let ble_clipboard_offer_tx = crate::clipboard_sync::spawn_image_offer_consumer();
        // File-transfer pills (incoming pull + outgoing push) + receive-consent
        // action router. Self-contained — only needs the live-activity channel.
        crate::worker_transfers::wire_transfer_indicators(ble_live_tx.clone());
        // Wi-Fi Direct: the phone offered a P2P group → join it + pull fast.
        {
            let app = app.clone();
            vortex_l3_daemon::core::wifi_direct::set_hook(Box::new(move |ssid, pass| {
                crate::lan_wifi_direct::on_wifi_direct_offer(app.clone(), ssid, pass);
            }));
        }

        // (1) Heartbeat: tick every 12 s, OR immediately when notified.
        //     Either way, a single Mutex serializes with the mDNS-wake
        //     and manual paths.
        lan::spawn_heartbeat(app.clone(), identity.clone(), peer_store.clone(), auto_lock.clone(), switch_orchestrator.clone(), session_writers.clone(), media_store.clone(), last_call_phase.clone(), media_watch.clone(), media_in_call.clone(), adapter.clone(), last_reconnect_at.clone(), sync_nudge.clone(), ble_audio_writers.clone());

        // Power watcher: edge-detect the laptop's charging flag + battery
        // level from sysfs every 2 s and, on a real change, nudge the
        // heartbeat to push the new state to the phone immediately. Cheap
        // (two tiny file reads) and fully portable — no UPower dependency.
        lan::spawn_power_watcher(sync_nudge.clone());

        // Locked-hint watcher: pushes fresh state to the phone the moment
        // the lock screen flips (remote command or local Super+L), so the
        // phone's lock icon doesn't sit stale until the next beat.
        lan::spawn_locked_watch(sync_nudge.clone());

        // Proximity auto-lock/unlock (both toggles opt-in, Settings page).
        crate::proximity::spawn_proximity_watch(
            ble_audio_writers.clone(),
            adapter.clone(),
            peer_store.clone(),
        );

        // Clipboard history watcher (P1) — polls wl-paste, persists to
        // ~/.cache/vortex/clipboard, feeds the Super+V popup.
        crate::clipboard::spawn_clipboard_watcher(app.clone());

        // Pin BLE discovery to the LE transport ONCE, up front. bluer
        // reads the stored discovery filter every time it (re)starts a
        // discovery session, and it only sends the filter on the FIRST
        // StartDiscovery of a shared session — so any per-scan attempt to
        // change it loses a race when a scan is already active (returns
        // DiscoveryActive). Storing LE here, before any scanner spawns,
        // guarantees every session (presence reconnect, UI pair scan,
        // even two that race at boot) is LE-only. Why it matters: a
        // *general* (dual-mode) discovery runs a fixed ~10.24 s BR/EDR
        // Inquiry that hogs the single radio and starved every LE connect
        // to ~10 s (root-caused via btmon). We never use BR/EDR discovery
        // for Vortex; the earbuds picker re-asserts Auto for its own scan.
        if let Err(e) = adapter
            .set_discovery_filter(bluer::DiscoveryFilter {
                transport: bluer::DiscoveryTransport::Le,
                ..Default::default()
            })
            .await
        {
            tracing::warn!("could not pin LE-only discovery filter at startup: {e}");
        } else {
            tracing::info!("BLE discovery pinned to LE-only transport (no BR/EDR inquiry)");
        }

        // (1b) BLE persistent listener (P2.13). Independent of the LAN
        //      reconnect loop above — owns its own GATT connection so
        //      AUDIO_OP frames (notably call-start) reach us in ~200 ms
        //      instead of waiting for the 12 s LAN heartbeat. Runs only
        //      when a trusted peer exists; restarts itself on disconnect.
        {
            let ble_adapter = adapter.clone();
            let ble_identity = identity.clone();
            let ble_peer_store = peer_store.clone();
            let ble_orch = switch_orchestrator.clone();
            let ble_media = media_store.clone();
            let ble_writers = ble_audio_writers.clone();
            let ble_state_tx = ble_state_tx.clone();
            let ble_notif_tx = ble_notif_tx.clone();
            let ble_live_tx = ble_live_tx.clone();
            let ble_icon_tx = ble_icon_tx.clone();
            let ble_call_tx = ble_call_tx.clone();
            let ble_contacts_tx = ble_contacts_tx.clone();
            let ble_call_log_tx = ble_call_log_tx.clone();
            let ble_sms_tx = ble_sms_tx.clone();
            let ble_sms_thread_tx = ble_sms_thread_tx.clone();
            let ble_clipboard_tx = ble_clipboard_tx.clone();
            let ble_clipboard_image_tx = ble_clipboard_image_tx.clone();
            let ble_clipboard_offer_tx = ble_clipboard_offer_tx.clone();
            let ble_handoff_tx = ble_handoff_tx.clone();
            let ble_notes_tx = ble_notes_tx.clone();
            let ble_notif_writer = ble_notif_writer.clone();
            let ble_clipboard_writer = ble_clipboard_writer.clone();
            let ble_clipboard_image_writer = ble_clipboard_image_writer.clone();
            let ble_call_writer = ble_call_writer.clone();
            let ble_sealed_writer = ble_sealed_writer.clone();
            let ble_nudge = ble_retry_nudge.clone();
            tokio::spawn(async move {
                run_ble_persistent_loop(
                    ble_adapter,
                    ble_identity,
                    ble_peer_store,
                    ble_orch,
                    ble_media,
                    ble_writers,
                    ble_state_tx,
                    ble_notif_tx,
                    ble_live_tx,
                    ble_icon_tx,
                    ble_call_tx,
                    ble_contacts_tx,
                    ble_call_log_tx,
                    ble_sms_tx,
                    ble_sms_thread_tx,
                    ble_clipboard_tx,
                    ble_clipboard_image_tx,
                    ble_clipboard_offer_tx,
                    ble_handoff_tx,
                    ble_notes_tx,
                    ble_notif_writer,
                    ble_clipboard_writer,
                    ble_clipboard_image_writer,
                    ble_call_writer,
                    ble_sealed_writer,
                    ble_nudge,
                )
                .await;
            });
        }

        // (2) Event-driven wake-up: a long-lived mDNS browse fires the
        //     moment the phone announces itself (e.g. just woke up,
        //     just connected to Wi-Fi). We pounce on the first matching
        //     resolve so the user doesn't wait the full 12 s tick.
        //     Cheap: the auto_lock prevents overlap with the heartbeat.
        if let Ok(mut mdns_rx) =
            vortex_l3_daemon::core::lan::discovery::watch_candidates()
        {
            let auto_app = app.clone();
            let auto_identity = identity.clone();
            let auto_peer_store = peer_store.clone();
            let auto_lock_clone = auto_lock.clone();
            let auto_orch = switch_orchestrator.clone();
            let auto_writers = session_writers.clone();
            let auto_media = media_store.clone();
            let auto_last_phase = last_call_phase.clone();
            let auto_media_watch = media_watch.clone();
            let auto_media_in_call = media_in_call.clone();
            let auto_adapter = adapter.clone();
            let auto_ble_writers = ble_audio_writers.clone();
            let mdns_last_reconnect = last_reconnect_at.clone();
            tokio::spawn(async move {
                while let Some(_cand) = mdns_rx.recv().await {
                    // Debounce: try-lock so multiple resolves within
                    // one cycle collapse into a single reconnect.
                    let g = match auto_lock_clone.try_lock() {
                        Ok(g) => g,
                        Err(_) => continue,
                    };
                    // Cooldown: mdns-sd re-resolves the service every
                    // few seconds even while we're already connected.
                    // Skip if any path reconnected within the cooldown
                    // so those re-resolves don't snowball into a fresh
                    // TCP+IK storm (the wedge documented at auto_lock).
                    {
                        let last = mdns_last_reconnect.lock().await;
                        if let Some(t) = *last {
                            if t.elapsed() < MDNS_RECONNECT_COOLDOWN {
                                drop(g);
                                continue;
                            }
                        }
                    }
                    let have_trust = {
                        let store = auto_peer_store.clone();
                        tokio::task::spawn_blocking(move || {
                            !store.list().unwrap_or_default().is_empty()
                        })
                        .await
                        .unwrap_or(false)
                    };
                    if have_trust {
                        tracing::info!("mDNS wake-up: triggering immediate reconnect");
                        let ble_live = !auto_ble_writers.lock().await.is_empty();
                        let _ = try_lan_reconnect(
                            &auto_app,
                            &auto_identity,
                            auto_peer_store.clone(),
                            Some(auto_orch.clone()),
                            Some(auto_writers.clone()),
                            Some(auto_media.clone()),
                            Some(auto_last_phase.clone()),
                            ble_live,
                            Some(auto_adapter.clone()),
                            Some(auto_media_watch.clone()),
                            Some(auto_media_in_call.clone()),
                        )
                        .await;
                        *mdns_last_reconnect.lock().await =
                            Some(tokio::time::Instant::now());
                    }
                    drop(g);
                }
            });
        }

        // ----- Command loop -----
        // Handle to the most recent in-flight pairable scan. A pairing
        // connect needs a quiet radio (an active discovery contends with
        // connection establishment and dragged the connect out to ~10 s,
        // mirroring the reconnect case), so when the user taps Pair we
        // abort+await this first.
        let ctx = worker_ctx::WorkerCtx {
            app: app.clone(),
            adapter: adapter.clone(),
            identity: identity.clone(),
            peer_store: peer_store.clone(),
            switch_orchestrator: switch_orchestrator.clone(),
            session_writers: session_writers.clone(),
        };
        let mut active_scan: Option<tokio::task::JoinHandle<()>> = None;
        loop {
            let cmd = match cmd_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(c) => c,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            // Thin dispatcher: each arm delegates to its feature's command
            // handler (cmd_pairing / cmd_earbuds / mirror) so run_worker stays
            // small as features are added.
            match cmd {
                UiCmd::Scan => cmd_pairing::scan(&ctx, &mut active_scan),
                UiCmd::Pair(addr_str) => cmd_pairing::pair(&ctx, addr_str, &mut active_scan).await,
                UiCmd::ForgetPeer(hex_str) => cmd_pairing::forget_peer(&ctx, hex_str).await,
                UiCmd::ForgetAll => cmd_pairing::forget_all(&ctx).await,
                UiCmd::RefreshState => cmd_earbuds::refresh_state(&ctx).await,
                UiCmd::RefreshLocalEarbuds => cmd_earbuds::refresh_local_earbuds(&ctx).await,
                UiCmd::RequestEarbudsSwitch { peer_static_pub, mac } => {
                    cmd_earbuds::request_switch(&ctx, peer_static_pub, mac).await
                }
                UiCmd::SendEarbudsClaim { peer_static_pub, mac } => {
                    cmd_earbuds::send_claim(&ctx, peer_static_pub, mac).await
                }
                UiCmd::ToggleEarbuds => cmd_earbuds::toggle_earbuds(&ctx).await,
                UiCmd::StartMirror { width, height, fps, bitrate } => {
                    crate::mirror::handle_start_cmd(&ctx, width, height, fps, bitrate).await
                }
                UiCmd::StopMirror => crate::mirror::handle_stop_cmd(),
            }
        }
    });
}
