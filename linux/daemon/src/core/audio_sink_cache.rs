//! Steady-state cache of the current PulseAudio / PipeWire `bluez_*` sink
//! names, kept fresh by a single long-lived `pactl subscribe` stream
//! instead of forking `pactl list short sinks` on every poll.
//!
//! **Why.** While the laptop holds the buds, `audio_active` is called on
//! every reconcile tick — `media_watch` polls at 200ms and the heartbeat
//! loop checks too. Each call used to fork a `pactl` subprocess just to
//! answer "is the bluez sink still there?": ~5 forks/s, indefinitely,
//! while the laptop is doing nothing but playing music. Wasted CPU and
//! battery.
//!
//! **Fix.** Mirror the `pactl subscribe` approach the codebase already
//! uses in `wait_audio_connected` (63eeaee): one background task owns a
//! `pactl subscribe` stream, re-reads `pactl list short sinks` only when a
//! sink-related event fires (debounced) plus a slow backstop, and
//! publishes the sink names into a shared snapshot. Readers consult the
//! snapshot with zero forks.
//!
//! **Safety.** Correctness never depends on the cache: until the updater
//! has done its first successful probe — or if `pactl subscribe` can't be
//! started at all — readers fall back to a direct one-shot fork (the exact
//! pre-cache behaviour). So the worst case is "no optimization", never
//! "wrong answer".

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use tracing::{debug, warn};

/// Max staleness backstop. Even with no subscribe events the snapshot is
/// re-probed at least this often — insurance against a coalesced/missed
/// event. Generous on purpose: sink changes almost always fire an event,
/// so this is a safety net, not the primary refresh path.
const BACKSTOP: Duration = Duration::from_secs(5);
/// Debounce window: a profile switch (HFP↔A2DP) or connect fires a burst
/// of sink events; collapse the burst into a single re-probe.
const DEBOUNCE: Duration = Duration::from_millis(80);
/// Backoff before re-subscribing after the stream ends / fails to start.
const RESUBSCRIBE_BACKOFF: Duration = Duration::from_millis(500);
/// Longer backoff when `pactl subscribe` can't even be spawned (no pactl).
const SPAWN_FAIL_BACKOFF: Duration = Duration::from_secs(10);

struct SinkCache {
    /// Current `bluez_*` sink names (the `<name>` column of
    /// `pactl list short sinks`, filtered to those starting with "bluez").
    sinks: RwLock<Vec<String>>,
    /// True once the updater has done at least one successful probe and the
    /// subscribe stream is live. Cleared while (re)subscribing so readers
    /// fork directly in the gap rather than trusting a stale snapshot.
    healthy: AtomicBool,
}

fn cache() -> &'static Arc<SinkCache> {
    static CACHE: OnceLock<Arc<SinkCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let c = Arc::new(SinkCache {
            sinks: RwLock::new(Vec::new()),
            healthy: AtomicBool::new(false),
        });
        spawn_updater(c.clone());
        c
    })
}

/// True if any current `bluez_*` sink name contains one of `needles`.
///
/// Consults the cached snapshot (zero forks) when the updater is healthy;
/// otherwise falls back to a direct one-shot `pactl list short sinks` fork
/// so the answer is always correct even before the cache warms up.
pub async fn has_bluez_sink_for(needles: &[&str]) -> bool {
    let c = cache();
    if c.healthy.load(Ordering::Acquire) {
        let sinks = c.sinks.read().unwrap_or_else(|e| e.into_inner());
        return sinks
            .iter()
            .any(|name| needles.iter().any(|n| name.contains(n)));
    }
    // Cache not ready (first call, or updater unavailable) — direct probe.
    probe_bluez_sinks()
        .await
        .unwrap_or_default()
        .iter()
        .any(|name| needles.iter().any(|n| name.contains(n)))
}

/// One-shot `pactl list short sinks` fork → the `bluez_*` sink names.
/// `None` when pactl is unavailable or errored.
async fn probe_bluez_sinks() -> Option<Vec<String>> {
    let out = tokio::process::Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut names = Vec::new();
    for line in text.lines() {
        // `pactl list short sinks`: `<id>\t<name>\t<driver>\t<format>\t<state>`
        if let Some(name) = line.split('\t').nth(1) {
            if name.starts_with("bluez") {
                names.push(name.to_string());
            }
        }
    }
    Some(names)
}

fn store(c: &SinkCache, names: Vec<String>) {
    *c.sinks.write().unwrap_or_else(|e| e.into_inner()) = names;
    c.healthy.store(true, Ordering::Release);
}

fn spawn_updater(c: Arc<SinkCache>) {
    tokio::spawn(async move {
        loop {
            // Prime the snapshot before subscribing so we don't miss the
            // current state between subscribe start and the first event.
            if let Some(names) = probe_bluez_sinks().await {
                store(&c, names);
            }

            let mut child = match tokio::process::Command::new("pactl")
                .args(["subscribe"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
            {
                Ok(ch) => ch,
                Err(e) => {
                    warn!("sink-cache: pactl subscribe spawn failed: {e}; readers fork directly");
                    c.healthy.store(false, Ordering::Release);
                    tokio::time::sleep(SPAWN_FAIL_BACKOFF).await;
                    continue;
                }
            };
            let Some(stdout) = child.stdout.take() else {
                c.healthy.store(false, Ordering::Release);
                tokio::time::sleep(SPAWN_FAIL_BACKOFF).await;
                continue;
            };

            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            // Deadline for a debounced re-probe; `None` = nothing pending.
            let mut pending: Option<tokio::time::Instant> = None;
            let mut last_refresh = Instant::now();

            'stream: loop {
                tokio::select! {
                    biased;
                    line = lines.next_line() => {
                        match line {
                            // Only sink events are relevant; a sink line
                            // (re)arms the debounce deadline so a burst
                            // collapses into one re-probe after it settles.
                            Ok(Some(l)) => {
                                if l.contains("sink") {
                                    pending = Some(tokio::time::Instant::now() + DEBOUNCE);
                                }
                            }
                            // Stream ended / errored (pulse restart, pactl
                            // died) → resubscribe from the top.
                            Ok(None) => { debug!("sink-cache: subscribe stream ended; resubscribing"); break 'stream; }
                            Err(e) => { warn!("sink-cache: subscribe read error: {e}; resubscribing"); break 'stream; }
                        }
                    }
                    // Debounced re-probe fires once the burst settles.
                    _ = async {
                        match pending {
                            Some(t) => tokio::time::sleep_until(t).await,
                            None => std::future::pending::<()>().await,
                        }
                    } => {
                        pending = None;
                        if let Some(names) = probe_bluez_sinks().await {
                            store(&c, names);
                            last_refresh = Instant::now();
                        }
                    }
                    // Backstop: re-probe even with no events, in case one
                    // was coalesced/missed. Only when actually idle.
                    _ = tokio::time::sleep(BACKSTOP) => {
                        if last_refresh.elapsed() >= BACKSTOP {
                            if let Some(names) = probe_bluez_sinks().await {
                                store(&c, names);
                            }
                            last_refresh = Instant::now();
                        }
                    }
                }
            }

            // Don't trust a stale snapshot across a resubscribe gap.
            c.healthy.store(false, Ordering::Release);
            tokio::time::sleep(RESUBSCRIBE_BACKOFF).await;
        }
    });
}
