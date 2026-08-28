//! Secret Service-backed identity store per spec §3.2.
//!
//! Persists the entire 90-byte Identity Record as a single Secret Service
//! item. The schema name is `com.vortex.identity.v1`. The item is stored in
//! the user's default collection.

use std::collections::HashMap;

use secret_service::{EncryptionType, SecretService};

use super::{unlocked_default_collection, IdentityStore, StorageError, StorageResult};
use crate::core::crypto::x25519::{X25519Pub, X25519Sec, X25519SecBytes};
use crate::core::identity::{IdentityRecord, Platform, IDENTITY_VERSION};

const SCHEMA: &str = "com.vortex.identity.v1";
const LABEL: &str = "Vortex V1 identity";
const CONTENT_TYPE: &str = "application/x-vortex-identity-v1";

/// Runs all Secret Service work on the dedicated storage runtime via
/// [`super::secret_block_on`] — never on the ambient runtime, whose
/// workers a call-time burst can fully park (see `SECRET_RT` docs).
pub struct SecretServiceIdentityStore;

impl SecretServiceIdentityStore {
    /// Probe Secret Service availability. Fails closed if unreachable
    /// (per spec §3.2 — V1 requires platform secure storage).
    pub fn new() -> StorageResult<Self> {
        Self::block_on(async {
            let service = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("secret service connect: {e}")))?;
            service
                .get_default_collection()
                .await
                .map_err(|e| StorageError::Backend(format!("default collection: {e}")))?;
            Ok::<_, StorageError>(())
        })?;
        Ok(Self)
    }

    fn block_on<F, T>(fut: F) -> T
    where
        F: std::future::Future<Output = T> + Send,
        T: Send,
    {
        super::secret_block_on(fut)
    }
}

fn attrs() -> HashMap<&'static str, &'static str> {
    let mut a = HashMap::new();
    a.insert("schema", SCHEMA);
    a.insert("version", "1");
    a
}

fn encode_for_storage(record: &IdentityRecord) -> Vec<u8> {
    record.encode()
}

fn decode_from_storage(bytes: &[u8]) -> StorageResult<IdentityRecord> {
    if bytes.len() != 90 {
        return Err(StorageError::Backend(format!(
            "stored identity has wrong length: {} (expected 90)",
            bytes.len()
        )));
    }
    let version = bytes[0];
    if version != IDENTITY_VERSION {
        return Err(StorageError::Backend(format!(
            "stored identity has unknown version: {version:#04x}"
        )));
    }
    let mut device_id = [0u8; 16];
    device_id.copy_from_slice(&bytes[1..17]);
    let mut static_priv: X25519SecBytes = [0u8; 32];
    static_priv.copy_from_slice(&bytes[17..49]);
    let mut static_pub = [0u8; 32];
    static_pub.copy_from_slice(&bytes[49..81]);
    let created_at = u64::from_be_bytes(bytes[81..89].try_into().unwrap());
    let platform = Platform::from_byte(bytes[89])
        .ok_or_else(|| StorageError::Backend(format!("unknown platform byte: {}", bytes[89])))?;
    Ok(IdentityRecord {
        version,
        device_id,
        static_priv: X25519Sec(static_priv),
        static_pub: X25519Pub(static_pub),
        created_at,
        platform,
    })
}

impl IdentityStore for SecretServiceIdentityStore {
    fn save(&self, record: &IdentityRecord) -> StorageResult<()> {
        let payload = encode_for_storage(record);
        Self::block_on(async {
            let service = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let collection = unlocked_default_collection(&service).await?;
            collection
                .create_item(LABEL, attrs(), &payload, true /* replace */, CONTENT_TYPE)
                .await
                .map_err(|e| StorageError::Backend(format!("create_item: {e}")))?;
            Ok(())
        })
    }

    fn load(&self) -> StorageResult<IdentityRecord> {
        Self::block_on(async {
            let service = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let mut search = service
                .search_items(attrs())
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            let item = search.unlocked.pop().or_else(|| search.locked.pop());
            let item = match item {
                Some(i) => i,
                None => return Err(StorageError::NotFound),
            };
            // Unlock if needed.
            if item.is_locked().await.unwrap_or(false) {
                item.unlock()
                    .await
                    .map_err(|e| StorageError::Backend(format!("unlock: {e}")))?;
            }
            let secret = item
                .get_secret()
                .await
                .map_err(|e| StorageError::Backend(format!("get_secret: {e}")))?;
            decode_from_storage(&secret)
        })
    }

    fn forget(&self) -> StorageResult<()> {
        Self::block_on(async {
            let service = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(|e| StorageError::Backend(format!("connect: {e}")))?;
            let search = service
                .search_items(attrs())
                .await
                .map_err(|e| StorageError::Backend(format!("search: {e}")))?;
            for item in search.unlocked.iter().chain(search.locked.iter()) {
                item.delete()
                    .await
                    .map_err(|e| StorageError::Backend(format!("delete: {e}")))?;
            }
            Ok(())
        })
    }
}
