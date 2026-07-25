//! mDNS discovery per spec §5.4 and §8.1.

use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use tracing::{debug, info};

pub const TRUSTED_RUNTIME_SERVICE: &str = "_vortex._tcp.local.";

/// One mDNS-resolved Vortex peer endpoint.
#[derive(Debug, Clone)]
pub struct LanCandidate {
    pub instance_name: String,
    pub host: String,
    pub port: u16,
    pub addresses: Vec<std::net::IpAddr>,
}

/// Browse the trusted runtime service type and return the first observed
/// candidate (or none after `timeout`).
pub async fn discover_first(timeout: Duration) -> Result<Option<LanCandidate>, String> {
    let daemon = ServiceDaemon::new().map_err(|e| format!("mdns daemon: {e}"))?;
    let result = discover_first_inner(&daemon, timeout).await;
    // CRITICAL: ServiceDaemon spawns a background OS thread that keeps
    // running (polling sockets) when the handle is merely dropped — it
    // stops only on an explicit shutdown. discover_first runs on every
    // reconnect (12 s heartbeat + each mDNS wake-up), so a missing
    // shutdown leaked one spinning `mDNS_daemon` thread per call. Over
    // one session those piled up to ~150 threads burning >400 % CPU
    // (load average into the hundreds). Always tear the daemon down.
    let _ = daemon.shutdown();
    result
}

async fn discover_first_inner(
    daemon: &ServiceDaemon,
    timeout: Duration,
) -> Result<Option<LanCandidate>, String> {
    let receiver = daemon
        .browse(TRUSTED_RUNTIME_SERVICE)
        .map_err(|e| format!("browse: {e}"))?;
    info!("browsing {}", TRUSTED_RUNTIME_SERVICE);

    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        let remaining = deadline - now;
        let event = match tokio::task::spawn_blocking({
            let receiver = receiver.clone();
            let to = remaining;
            move || receiver.recv_timeout(to)
        })
        .await
        {
            Ok(Ok(event)) => event,
            Ok(Err(_recv_err)) => return Ok(None),
            Err(join_err) => return Err(format!("mdns task: {join_err}")),
        };

        if let ServiceEvent::ServiceResolved(info) = event {
            let fullname = info.get_fullname().to_string();
            debug!("resolved {} -> {}:{}", fullname, info.get_hostname(), info.get_port());
            // Skip records whose instance name doesn't look like a
            // Vortex peer. Both sides now publish `vortex-<8-hex>`
            // (rotating); reject anything else so a rogue announcer
            // on the LAN can't burn our TCP-connect budget.
            if !looks_like_vortex_instance(&fullname) {
                debug!("ignoring non-vortex instance {fullname}");
                continue;
            }
            let addrs: Vec<std::net::IpAddr> = info.get_addresses().iter().copied().collect();
            if addrs.is_empty() {
                continue;
            }
            return Ok(Some(LanCandidate {
                instance_name: fullname,
                host: info.get_hostname().to_string(),
                port: info.get_port(),
                addresses: addrs,
            }));
        }
    }
}

/// Spawn a long-lived mDNS browse task. The returned channel emits a
/// fresh [`LanCandidate`] every time `_vortex._tcp.local.` is
/// (re-)resolved on the LAN — which happens whenever a peer wakes
/// up, re-announces itself, or rotates its instance tag. Pair this
/// with the periodic auto-reconnect heartbeat so the laptop pounces
/// on a phone wake-up the instant mDNS sees it.
///
/// The task lives until the receiver is dropped.
pub fn watch_candidates() -> Result<tokio::sync::mpsc::UnboundedReceiver<LanCandidate>, String> {
    let daemon = ServiceDaemon::new().map_err(|e| format!("mdns daemon: {e}"))?;
    let recv = daemon
        .browse(TRUSTED_RUNTIME_SERVICE)
        .map_err(|e| format!("browse: {e}"))?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        // Owning the daemon for the thread's lifetime keeps the
        // browser alive. We never call daemon.shutdown_browser()
        // because dropping is enough.
        let _keepalive = daemon;
        while let Ok(event) = recv.recv() {
            if let ServiceEvent::ServiceResolved(info) = event {
                let fullname = info.get_fullname().to_string();
                if !looks_like_vortex_instance(&fullname) {
                    continue;
                }
                let addrs: Vec<std::net::IpAddr> =
                    info.get_addresses().iter().copied().collect();
                if addrs.is_empty() {
                    continue;
                }
                let cand = LanCandidate {
                    instance_name: fullname,
                    host: info.get_hostname().to_string(),
                    port: info.get_port(),
                    addresses: addrs,
                };
                if tx.send(cand).is_err() {
                    break; // receiver dropped → unwind
                }
            }
        }
    });
    Ok(rx)
}

/// Returns true if `fullname` matches the V1 rotating instance shape:
/// `vortex-<16 lowercase hex chars>._vortex._tcp.local.`.
///
/// **Strict format (ChatGPT review #3).** The previous predicate
/// accepted any `vortex-*` prefix, which let a rogue mDNS publisher
/// announce `vortex-foo` and burn this side's reconnect budget on a
/// TCP+IK attempt that was guaranteed to fail. Tightening the regex
/// to the exact 16-hex rotating-token form rejects arbitrary names
/// at the discovery layer; the higher-layer PRS check in the
/// reconnect path then makes sure the token actually belongs to a
/// trusted peer's current ±1 bucket.
fn looks_like_vortex_instance(fullname: &str) -> bool {
    let lower = fullname.to_ascii_lowercase();
    if !lower.ends_with("._vortex._tcp.local.") {
        return false;
    }
    let instance = match lower.split_once('.') {
        Some((head, _)) => head,
        None => return false,
    };
    let Some(suffix) = instance.strip_prefix("vortex-") else {
        return false;
    };
    // Exactly 16 lowercase hex chars (8 bytes of presence token).
    suffix.len() == 16 && suffix.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
