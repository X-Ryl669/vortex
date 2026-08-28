//! Trusted Peer storage per spec §3.3 (V1 subset).
//!
//! V1 schema (encoded as a 72-byte blob):
//!   | 0  | 32 | peer_static_pub |
//!   | 32 | 32 | prs (secret)    |
//!   | 64 |  8 | paired_at u64 BE |
//!
//! For Phase 5d.1 we persist `peer_static_pub` + `prs` + `paired_at`. The
//! richer fields (`peer_id`, `peer_display_name`, `peer_platform`) require
//! the post-handshake identity exchange (§6.4.4 / §6.7.1) which is
//! deferred.

use std::collections::HashMap;
use std::sync::Mutex;

use secret_service::{EncryptionType, SecretService};

use super::{unlocked_default_collection, StorageError, StorageResult};

const SCHEMA: &str = "com.vortex.peer.v1";
const LABEL: &str = "Vortex V1 trusted peer";
const CONTENT_TYPE: &str = "application/x-vortex-peer-v1";

/// Schema for the per-peer monotonic reconnect counter (M6).
const COUNTER_SCHEMA: &str = "com.vortex.peer.counter.v1";
const COUNTER_LABEL: &str = "Vortex V1 reconnect counter";
const COUNTER_CONTENT_TYPE: &str = "application/x-vortex-peer-counter-v1";

/// Schema for the per-peer audio-op nonce (sender / outbound).
/// Persisted so a process restart cannot reset the counter — a
/// rollback would let a network attacker replay a captured
/// `AudioOpFrame` (e.g. an old `Done`) and confuse our state.
const AUDIO_OUT_NONCE_SCHEMA: &str = "com.vortex.peer.audio_out_nonce.v1";
const AUDIO_OUT_NONCE_LABEL: &str = "Vortex V1 audio-op outbound nonce";
const AUDIO_OUT_NONCE_CT: &str = "application/x-vortex-audio-out-nonce-v1";

/// Schema for the per-peer audio-op nonce (receiver / inbound).
/// Highest accepted nonce; we drop any frame with `<= in_nonce`.
const AUDIO_IN_NONCE_SCHEMA: &str = "com.vortex.peer.audio_in_nonce.v1";
const AUDIO_IN_NONCE_LABEL: &str = "Vortex V1 audio-op inbound nonce";
const AUDIO_IN_NONCE_CT: &str = "application/x-vortex-audio-in-nonce-v1";

/// Schema for the per-peer BT-bonded identity address (BD_ADDR string,
/// e.g. "74:15:75:27:AB:2E"). Stored after a successful BT bond during
/// pairing so the persistent reconnect loop can connect DIRECTLY to the
/// stable identity (no BLE scan — scanning conflicts with A2DP on the
/// shared radio). Absent on records that haven't bonded yet.
const BONDED_ADDR_SCHEMA: &str = "com.vortex.peer.bonded_addr.v1";
const BONDED_ADDR_LABEL: &str = "Vortex V1 BT-bonded identity address";
const BONDED_ADDR_CT: &str = "application/x-vortex-bonded-addr-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPeer {
    pub peer_static_pub: [u8; 32],
    pub prs: [u8; 32],
    pub paired_at: u64,
    /// Friendly device name exchanged during pairing. Absent on
    /// records created before the name-exchange landed.
    pub peer_name: Option<String>,
}

impl TrustedPeer {
    /// Encode as 72 bytes (v1) when no name is present, or 72 + utf8
    /// name bytes (v2). Backward-compatible: v1 readers reject longer
    /// records, but we never write v2 unless `peer_name` is Some.
    pub fn encode(&self) -> Vec<u8> {
        let name_bytes = self.peer_name.as_deref().unwrap_or("").as_bytes();
        let mut out = Vec::with_capacity(72 + name_bytes.len());
        out.extend_from_slice(&self.peer_static_pub);
        out.extend_from_slice(&self.prs);
        out.extend_from_slice(&self.paired_at.to_be_bytes());
        if !name_bytes.is_empty() {
            out.extend_from_slice(name_bytes);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> StorageResult<Self> {
        if bytes.len() < 72 {
            return Err(StorageError::Backend(format!(
                "trusted peer record too short: {} (need ≥72)",
                bytes.len()
            )));
        }
        let mut peer_static_pub = [0u8; 32];
        peer_static_pub.copy_from_slice(&bytes[0..32]);
        let mut prs = [0u8; 32];
        prs.copy_from_slice(&bytes[32..64]);
        let paired_at = u64::from_be_bytes(bytes[64..72].try_into().unwrap());
        // Sanitize on decode (ChatGPT review #9). Writers go through
        // `sanitize_peer_name`, but older / corrupted records (e.g.
        // pre-sanitizer build, on-disk corruption, future schema
        // skew) could still contain control chars or oversized
        // strings that the UI would render badly. Strip both here
        // so every read produces UI-safe text — defense in depth.
        let peer_name = if bytes.len() > 72 {
            std::str::from_utf8(&bytes[72..])
                .ok()
                .map(crate::core::pairing::handshake::sanitize_peer_name)
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        Ok(Self {
            peer_static_pub,
            prs,
            paired_at,
            peer_name,
        })
    }
}

pub trait PeerStore: Send + Sync {
    fn save(&self, peer: &TrustedPeer) -> StorageResult<()>;
    fn load(&self, peer_static_pub: &[u8; 32]) -> StorageResult<TrustedPeer>;
    fn list(&self) -> StorageResult<Vec<TrustedPeer>>;
    fn forget(&self, peer_static_pub: &[u8; 32]) -> StorageResult<()>;

    /// Per-peer monotonic reconnect counter (M6 audit fix). Defaults to
    /// zero before the first successful reconnect.
    ///
    /// On each successful IK exchange both sides bump and persist
    /// `max(local, peer_seen) + 1`. A peer counter strictly lower than
    /// the local counter is a backup-restore replay signal — logged at
    /// warn level; not hard-rejected because legitimate partial
    /// reconnects can desync. See trait doc on the Android side for
    /// the full rationale.
    fn load_counter(&self, _peer_static_pub: &[u8; 32]) -> StorageResult<u64> {
        Ok(0)
    }
    fn bump_counter(&self, _peer_static_pub: &[u8; 32], _peer_seen: u64) -> StorageResult<u64> {
        Ok(0)
    }

    /// BT-bonded identity address for this peer (BD_ADDR string), set
    /// after a successful bond during pairing. `None` until bonded. The
    /// persistent reconnect loop uses it to connect DIRECTLY (no BLE
    /// scan — scanning conflicts with A2DP on the shared radio, which is
    /// what made the handoff link flaky while audio streamed).
    fn load_bonded_addr(&self, _peer_static_pub: &[u8; 32]) -> StorageResult<Option<String>> {
        Ok(None)
    }
    fn save_bonded_addr(&self, _peer_static_pub: &[u8; 32], _addr: &str) -> StorageResult<()> {
        Ok(())
    }

    /// Atomically read-and-increment the outbound audio-op nonce for
    /// `peer_static_pub`. Returns the new value to embed in the next
    /// `AudioOpFrame`. Default 0 → first send carries nonce 1.
    fn next_audio_out_nonce(&self, _peer_static_pub: &[u8; 32]) -> StorageResult<u64> {
        Ok(0)
    }

    /// Highest inbound nonce we've accepted from this peer. The
    /// orchestrator rejects any incoming frame with `nonce <= this`.
    fn load_audio_in_nonce(&self, _peer_static_pub: &[u8; 32]) -> StorageResult<u64> {
        Ok(0)
    }

    /// Atomically accept `nonce` if and only if it's strictly greater
    /// than the previously-seen value. Returns Ok(true) when the
    /// caller may go on to dispatch the frame (we committed the new
    /// nonce); Ok(false) when the frame is a replay and must be
    /// dropped.
    ///
    /// This is the API that defends against the BLE+LAN duplicate-
    /// delivery race: both transports can hand the orchestrator the
    /// same frame in the same millisecond, and the prior `load + if
    /// nonce > seen + commit` shape let both readers see the old
    /// `seen` value and both pass the check. Implementations override
    /// this with a single mutex window around load-compare-commit so
    /// exactly one caller wins.
    ///
    /// Default impl exists for the no-op / in-memory stores — they
    /// don't have a shared mutex, so the default just does the same
    /// load+commit pair the orchestrator was doing inline. Real
    /// stores MUST override.
    fn try_accept_audio_in_nonce(
        &self,
        peer_static_pub: &[u8; 32],
        nonce: u64,
    ) -> StorageResult<bool> {
        let seen = self.load_audio_in_nonce(peer_static_pub)?;
        if nonce <= seen {
            return Ok(false);
        }
        self.commit_audio_in_nonce(peer_static_pub, nonce)?;
        Ok(true)
    }

    /// Persist a new accepted inbound nonce. The implementation MUST
    /// `max()` against the current value to be idempotent under
    /// concurrent frame arrival (BLE + LAN can both deliver the same
    /// frame; we accept whichever lands first and ignore the other).
    fn commit_audio_in_nonce(&self, _peer_static_pub: &[u8; 32], _nonce: u64) -> StorageResult<()> {
        Ok(())
    }
}

/// Runs all Secret Service work on the dedicated storage runtime via
/// [`super::secret_block_on`] (same approach as
/// [`super::secret_service::SecretServiceIdentityStore`]) — never on the
/// ambient runtime, whose workers a call-time burst can fully park (the
/// 2026-06-11 earbuds-switch freeze).
///
/// A `Mutex` serializes every call so a burst of concurrent saves /
/// reconnect attempts cannot leave the Secret Service with duplicate
/// items for the same peer_static_pub or race the create-then-search
/// path.
pub struct SecretServicePeerStore {
    lock: Mutex<()>,
}

impl SecretServicePeerStore {
    pub fn new() -> StorageResult<Self> {
        Self::block_on(async {
            let s = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            s.get_default_collection()
                .await
                .map_err(|e| StorageError::Backend(format!("collection: {e}")))?;
            Ok::<_, StorageError>(())
        })?;
        Ok(Self { lock: Mutex::new(()) })
    }

    fn block_on<F, T>(fut: F) -> T
    where
        F: std::future::Future<Output = T> + Send,
        T: Send,
    {
        super::secret_block_on(fut)
    }

    fn attrs_for(peer_pub_hex: &str) -> HashMap<String, String> {
        let mut a = HashMap::new();
        a.insert("schema".to_string(), SCHEMA.to_string());
        a.insert("peer_static_pub".to_string(), peer_pub_hex.to_string());
        a
    }

    fn schema_only_attrs() -> HashMap<&'static str, &'static str> {
        let mut a = HashMap::new();
        a.insert("schema", SCHEMA);
        a
    }

    fn counter_attrs_for(peer_pub_hex: &str) -> HashMap<String, String> {
        let mut a = HashMap::new();
        a.insert("schema".to_string(), COUNTER_SCHEMA.to_string());
        a.insert("peer_static_pub".to_string(), peer_pub_hex.to_string());
        a
    }

    fn audio_out_attrs_for(peer_pub_hex: &str) -> HashMap<String, String> {
        let mut a = HashMap::new();
        a.insert("schema".to_string(), AUDIO_OUT_NONCE_SCHEMA.to_string());
        a.insert("peer_static_pub".to_string(), peer_pub_hex.to_string());
        a
    }

    fn audio_in_attrs_for(peer_pub_hex: &str) -> HashMap<String, String> {
        let mut a = HashMap::new();
        a.insert("schema".to_string(), AUDIO_IN_NONCE_SCHEMA.to_string());
        a.insert("peer_static_pub".to_string(), peer_pub_hex.to_string());
        a
    }

    fn bonded_addr_attrs_for(peer_pub_hex: &str) -> HashMap<String, String> {
        let mut a = HashMap::new();
        a.insert("schema".to_string(), BONDED_ADDR_SCHEMA.to_string());
        a.insert("peer_static_pub".to_string(), peer_pub_hex.to_string());
        a
    }

    /// Read-modify-write helper for a single 8-byte u64 secret slot.
    /// Used by both the counter and the audio-nonce paths. `op`
    /// receives the current value (or 0 if absent) and returns the
    /// new value to store.
    fn rmw_u64_slot(
        &self,
        attrs_owned: HashMap<String, String>,
        label: &'static str,
        content_type: &'static str,
        op: impl FnOnce(u64) -> u64 + Send,
    ) -> StorageResult<u64> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| StorageError::Backend("peer store mutex poisoned".into()))?;
        let attrs_ref: HashMap<&str, &str> = attrs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        Self::block_on(async {
            let svc = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let coll = unlocked_default_collection(&svc).await?;
            let mut search = svc
                .search_items(attrs_ref.clone())
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            let current: u64 = if let Some(item) =
                search.unlocked.pop().or_else(|| search.locked.pop())
            {
                if item.is_locked().await.unwrap_or(false) {
                    item.unlock()
                        .await
                        .map_err(|e| StorageError::Backend(format!("unlock: {e}")))?;
                }
                let bytes = item
                    .get_secret()
                    .await
                    .map_err(|e| StorageError::Backend(format!("get_secret: {e}")))?;
                if bytes.len() == 8 {
                    u64::from_be_bytes(bytes[..8].try_into().unwrap())
                } else {
                    0
                }
            } else {
                0
            };
            let next = op(current);
            // Remove old item(s) so create_item produces exactly one
            // record (Secret Service has no atomic upsert).
            let mut existing = svc
                .search_items(attrs_ref)
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            for item in existing.unlocked.drain(..).chain(existing.locked.drain(..)) {
                let _ = item.delete().await;
            }
            coll.create_item(
                label,
                attrs_owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(),
                &next.to_be_bytes(),
                /*replace=*/ false,
                content_type,
            )
            .await
            .map_err(|e| StorageError::Backend(format!("create_item: {e}")))?;
            Ok::<u64, StorageError>(next)
        })
    }
}

impl PeerStore for SecretServicePeerStore {
    fn save(&self, peer: &TrustedPeer) -> StorageResult<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| StorageError::Backend("peer store mutex poisoned".into()))?;
        let payload = peer.encode().to_vec();
        let pub_hex = hex::encode(peer.peer_static_pub);
        let attrs_owned = Self::attrs_for(&pub_hex);
        let attrs_ref: HashMap<&str, &str> = attrs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        Self::block_on(async {
            let svc = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let coll = unlocked_default_collection(&svc).await?;
            // Dedupe: explicitly remove any prior items for this
            // peer_static_pub before creating. The `replace=true` flag
            // on create_item alone is unreliable across Secret Service
            // implementations (gnome-keyring vs kwallet handle the
            // "match by attributes" semantics differently); explicit
            // search + delete is portable.
            let prior = svc
                .search_items(attrs_ref.clone())
                .await
                .map_err(|e| StorageError::Backend(format!("dedupe search: {e}")))?;
            for item in prior.unlocked.iter().chain(prior.locked.iter()) {
                item.delete()
                    .await
                    .map_err(|e| StorageError::Backend(format!("dedupe delete: {e}")))?;
            }
            coll.create_item(LABEL, attrs_ref, &payload, true, CONTENT_TYPE)
                .await
                .map_err(|e| StorageError::Backend(format!("create_item: {e}")))?;
            Ok(())
        })
    }

    fn load(&self, peer_static_pub: &[u8; 32]) -> StorageResult<TrustedPeer> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| StorageError::Backend("peer store mutex poisoned".into()))?;
        let pub_hex = hex::encode(peer_static_pub);
        let attrs_owned = Self::attrs_for(&pub_hex);
        let attrs_ref: HashMap<&str, &str> = attrs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        Self::block_on(async {
            let svc = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let mut search = svc
                .search_items(attrs_ref)
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            let item = search.unlocked.pop().or_else(|| search.locked.pop());
            let item = match item {
                Some(i) => i,
                None => return Err(StorageError::NotFound),
            };
            if item.is_locked().await.unwrap_or(false) {
                item.unlock()
                    .await
                    .map_err(|e| StorageError::Backend(format!("unlock: {e}")))?;
            }
            let bytes = item
                .get_secret()
                .await
                .map_err(|e| StorageError::Backend(format!("get_secret: {e}")))?;
            TrustedPeer::decode(&bytes)
        })
    }

    fn list(&self) -> StorageResult<Vec<TrustedPeer>> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| StorageError::Backend("peer store mutex poisoned".into()))?;
        Self::block_on(async {
            let svc = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let search = svc
                .search_items(Self::schema_only_attrs())
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            let mut peers = Vec::new();
            for item in search.unlocked.iter().chain(search.locked.iter()) {
                if item.is_locked().await.unwrap_or(false) {
                    item.unlock()
                        .await
                        .map_err(|e| StorageError::Backend(format!("unlock: {e}")))?;
                }
                let bytes = item
                    .get_secret()
                    .await
                    .map_err(|e| StorageError::Backend(format!("get_secret: {e}")))?;
                if let Ok(peer) = TrustedPeer::decode(&bytes) {
                    peers.push(peer);
                }
            }
            Ok(peers)
        })
    }

    fn forget(&self, peer_static_pub: &[u8; 32]) -> StorageResult<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| StorageError::Backend("peer store mutex poisoned".into()))?;
        let pub_hex = hex::encode(peer_static_pub);
        let attrs_owned = Self::attrs_for(&pub_hex);
        let attrs_ref: HashMap<&str, &str> = attrs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        Self::block_on(async {
            let svc = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let search = svc
                .search_items(attrs_ref)
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            let trust_count = search.unlocked.len() + search.locked.len();
            for item in search.unlocked.iter().chain(search.locked.iter()) {
                item.delete()
                    .await
                    .map_err(|e| StorageError::Backend(format!("delete: {e}")))?;
            }
            // Cascade: drop every other schema we hold for this peer
            // (counter + audio_out_nonce + audio_in_nonce). A partial
            // forget would leave stale nonces that survive into a
            // future re-pair and cause replay-protection mismatches.
            let cascades = [
                ("counter", Self::counter_attrs_for(&pub_hex)),
                ("audio_out_nonce", Self::audio_out_attrs_for(&pub_hex)),
                ("audio_in_nonce", Self::audio_in_attrs_for(&pub_hex)),
                ("bonded_addr", Self::bonded_addr_attrs_for(&pub_hex)),
            ];
            let mut cascade_counts: Vec<(&str, usize)> = Vec::new();
            for (label, attrs_owned) in &cascades {
                let attrs_ref: HashMap<&str, &str> = attrs_owned
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                if let Ok(cs) = svc.search_items(attrs_ref).await {
                    let n = cs.unlocked.len() + cs.locked.len();
                    cascade_counts.push((*label, n));
                    for item in cs.unlocked.iter().chain(cs.locked.iter()) {
                        let _ = item.delete().await;
                    }
                }
            }
            tracing::info!(
                trust = trust_count,
                cascades = ?cascade_counts,
                "peer_store.forget cleared {} entries",
                &pub_hex[..16],
            );
            Ok(())
        })
    }

    fn load_counter(&self, peer_static_pub: &[u8; 32]) -> StorageResult<u64> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| StorageError::Backend("peer store mutex poisoned".into()))?;
        let pub_hex = hex::encode(peer_static_pub);
        let attrs_owned = Self::counter_attrs_for(&pub_hex);
        let attrs_ref: HashMap<&str, &str> = attrs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        Self::block_on(async {
            let svc = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let mut search = svc
                .search_items(attrs_ref)
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            let item = match search.unlocked.pop().or_else(|| search.locked.pop()) {
                Some(i) => i,
                None => return Ok(0u64),
            };
            if item.is_locked().await.unwrap_or(false) {
                item.unlock()
                    .await
                    .map_err(|e| StorageError::Backend(format!("unlock: {e}")))?;
            }
            let bytes = item
                .get_secret()
                .await
                .map_err(|e| StorageError::Backend(format!("get_secret: {e}")))?;
            if bytes.len() != 8 {
                return Ok(0u64);
            }
            Ok(u64::from_be_bytes(bytes[..8].try_into().unwrap()))
        })
    }

    fn load_bonded_addr(&self, peer_static_pub: &[u8; 32]) -> StorageResult<Option<String>> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| StorageError::Backend("peer store mutex poisoned".into()))?;
        let pub_hex = hex::encode(peer_static_pub);
        let attrs_owned = Self::bonded_addr_attrs_for(&pub_hex);
        let attrs_ref: HashMap<&str, &str> = attrs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        Self::block_on(async {
            let svc = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let mut search = svc
                .search_items(attrs_ref)
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            let Some(item) = search.unlocked.pop().or_else(|| search.locked.pop()) else {
                return Ok(None);
            };
            if item.is_locked().await.unwrap_or(false) {
                item.unlock()
                    .await
                    .map_err(|e| StorageError::Backend(format!("unlock: {e}")))?;
            }
            let bytes = item
                .get_secret()
                .await
                .map_err(|e| StorageError::Backend(format!("get_secret: {e}")))?;
            Ok(String::from_utf8(bytes).ok().filter(|s| !s.is_empty()))
        })
    }

    fn save_bonded_addr(&self, peer_static_pub: &[u8; 32], addr: &str) -> StorageResult<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| StorageError::Backend("peer store mutex poisoned".into()))?;
        let pub_hex = hex::encode(peer_static_pub);
        let attrs_owned = Self::bonded_addr_attrs_for(&pub_hex);
        let attrs_ref: HashMap<&str, &str> = attrs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let addr_owned = addr.to_string();
        Self::block_on(async {
            let svc = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let coll = unlocked_default_collection(&svc).await?;
            // Single-record upsert: drop any existing then create one.
            let mut existing = svc
                .search_items(attrs_ref)
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            for item in existing.unlocked.drain(..).chain(existing.locked.drain(..)) {
                let _ = item.delete().await;
            }
            coll.create_item(
                BONDED_ADDR_LABEL,
                attrs_owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(),
                addr_owned.as_bytes(),
                /*replace=*/ false,
                BONDED_ADDR_CT,
            )
            .await
            .map_err(|e| StorageError::Backend(format!("create_item: {e}")))?;
            Ok(())
        })
    }

    fn bump_counter(&self, peer_static_pub: &[u8; 32], peer_seen: u64) -> StorageResult<u64> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| StorageError::Backend("peer store mutex poisoned".into()))?;
        let pub_hex = hex::encode(peer_static_pub);
        let attrs_owned = Self::counter_attrs_for(&pub_hex);
        let attrs_ref: HashMap<&str, &str> = attrs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        Self::block_on(async {
            let svc = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let coll = unlocked_default_collection(&svc).await?;
            // Read current.
            let mut search = svc
                .search_items(attrs_ref.clone())
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            let current: u64 = if let Some(item) =
                search.unlocked.pop().or_else(|| search.locked.pop())
            {
                if item.is_locked().await.unwrap_or(false) {
                    item.unlock()
                        .await
                        .map_err(|e| StorageError::Backend(format!("unlock: {e}")))?;
                }
                let bytes = item
                    .get_secret()
                    .await
                    .map_err(|e| StorageError::Backend(format!("get_secret: {e}")))?;
                let _ = item.delete().await;
                if bytes.len() == 8 {
                    u64::from_be_bytes(bytes[..8].try_into().unwrap())
                } else {
                    0
                }
            } else {
                0
            };
            let next = current.max(peer_seen).saturating_add(1);
            let payload = next.to_be_bytes().to_vec();
            coll.create_item(
                COUNTER_LABEL,
                attrs_ref,
                &payload,
                true,
                COUNTER_CONTENT_TYPE,
            )
            .await
            .map_err(|e| StorageError::Backend(format!("create counter: {e}")))?;
            Ok(next)
        })
    }

    fn next_audio_out_nonce(&self, peer_static_pub: &[u8; 32]) -> StorageResult<u64> {
        let attrs = Self::audio_out_attrs_for(&hex::encode(peer_static_pub));
        self.rmw_u64_slot(
            attrs,
            AUDIO_OUT_NONCE_LABEL,
            AUDIO_OUT_NONCE_CT,
            |current| current.saturating_add(1),
        )
    }

    fn load_audio_in_nonce(&self, peer_static_pub: &[u8; 32]) -> StorageResult<u64> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| StorageError::Backend("peer store mutex poisoned".into()))?;
        let pub_hex = hex::encode(peer_static_pub);
        let attrs_owned = Self::audio_in_attrs_for(&pub_hex);
        let attrs_ref: HashMap<&str, &str> = attrs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        Self::block_on(async {
            let svc = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let mut search = svc
                .search_items(attrs_ref)
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            let item = match search.unlocked.pop().or_else(|| search.locked.pop()) {
                Some(i) => i,
                None => return Ok(0u64),
            };
            if item.is_locked().await.unwrap_or(false) {
                item.unlock()
                    .await
                    .map_err(|e| StorageError::Backend(format!("unlock: {e}")))?;
            }
            let bytes = item
                .get_secret()
                .await
                .map_err(|e| StorageError::Backend(format!("get_secret: {e}")))?;
            if bytes.len() != 8 {
                return Ok(0u64);
            }
            Ok(u64::from_be_bytes(bytes[..8].try_into().unwrap()))
        })
    }

    fn commit_audio_in_nonce(&self, peer_static_pub: &[u8; 32], nonce: u64) -> StorageResult<()> {
        let attrs = Self::audio_in_attrs_for(&hex::encode(peer_static_pub));
        // max() against current so a duplicate frame delivered via both
        // BLE and LAN can't write a smaller nonce.
        let _ = self.rmw_u64_slot(
            attrs,
            AUDIO_IN_NONCE_LABEL,
            AUDIO_IN_NONCE_CT,
            |current| current.max(nonce),
        )?;
        Ok(())
    }

    /// Atomic accept-if-fresh: rmw_u64_slot holds the per-store mutex
    /// plus the secret-service round-trip lock across the read, compare,
    /// and (conditional) commit. Two concurrent callers therefore
    /// linearise — the first to take the lock decides; the second
    /// sees the freshly-committed nonce and returns false. Fixes the
    /// BLE+LAN duplicate-delivery race where both transports could
    /// drive the same frame into the orchestrator.
    fn try_accept_audio_in_nonce(
        &self,
        peer_static_pub: &[u8; 32],
        nonce: u64,
    ) -> StorageResult<bool> {
        let attrs = Self::audio_in_attrs_for(&hex::encode(peer_static_pub));
        // `rmw_u64_slot` runs the closure with the slot's current
        // value under the mutex; whatever the closure returns is
        // what gets persisted. We use an AtomicBool to ferry "did we
        // actually advance" back out — the closure can't return a
        // tuple because the slot is u64-typed (and it must be Send
        // for the dedicated-runtime hop, which rules out Cell).
        use std::sync::atomic::{AtomicBool, Ordering};
        let accepted = AtomicBool::new(false);
        self.rmw_u64_slot(
            attrs,
            AUDIO_IN_NONCE_LABEL,
            AUDIO_IN_NONCE_CT,
            |current| {
                if nonce > current {
                    accepted.store(true, Ordering::Relaxed);
                    nonce
                } else {
                    current
                }
            },
        )?;
        Ok(accepted.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trip() {
        let peer = TrustedPeer {
            peer_static_pub: [0xAA; 32],
            prs: [0xBB; 32],
            paired_at: 1_700_000_000,
            peer_name: None,
        };
        let encoded = peer.encode();
        assert_eq!(encoded.len(), 72);
        let decoded = TrustedPeer::decode(&encoded).unwrap();
        assert_eq!(decoded, peer);
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(TrustedPeer::decode(&[0u8; 71]).is_err());
        // ≥72 bytes is accepted — bytes past 72 are treated as the
        // optional utf-8 device name (v2 format).
        assert!(TrustedPeer::decode(&[0u8; 72]).is_ok());
    }

    #[test]
    fn round_trip_with_name() {
        let peer = TrustedPeer {
            peer_static_pub: [0xAA; 32],
            prs: [0xBB; 32],
            paired_at: 1_700_000_000,
            peer_name: Some("zoyirjon-Blade".to_string()),
        };
        let encoded = peer.encode();
        let decoded = TrustedPeer::decode(&encoded).unwrap();
        assert_eq!(decoded, peer);
        assert_eq!(decoded.peer_name.as_deref(), Some("zoyirjon-Blade"));
    }
}
