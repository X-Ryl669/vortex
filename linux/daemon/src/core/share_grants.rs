//! Files the user has explicitly shared with the phone, addressable by an
//! opaque token so the phone can pull them through the ranged-read protocol.
//!
//! The laptop half of Kotlin `ShareGrants`, and authorised the same way: a
//! shared file is usually not under any served root (`fs-roots.conf`), and it
//! does not need to be — the *act of sharing* is the authorisation. So it gets
//! its own narrow gate: exactly the files the user sent, addressed by a token
//! that cannot be guessed, and nothing else.
//!
//! The path behind a grant is chosen by THIS side, never by the peer: the
//! peer only ever sends the token back. That is why a grant needs no root
//! check — there is no peer-supplied path to canonicalise.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use rand::RngCore;

/// Address prefix for a granted file, as the peer sends it in an `OPEN`.
/// Same spelling as Kotlin `FsRoots.SHARE_PREFIX`.
pub const SHARE_PREFIX: &str = "share:";

/// One shared file: what to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub path: PathBuf,
}

static GRANTS: Mutex<Option<HashMap<String, Grant>>> = Mutex::new(None);

/// Register `path` as shared; returns the token the phone pulls it by.
///
/// No count cap, unlike the phone's: a grant here costs a path, and every
/// grant is revoked when the batch it belongs to ends (delivered, declined or
/// failed), so the table only ever holds the batch in flight.
pub fn grant(path: PathBuf) -> String {
    let token = random_token();
    if let Ok(mut g) = GRANTS.lock() {
        g.get_or_insert_with(HashMap::new)
            .insert(token.clone(), Grant { path });
    }
    token
}

/// The grant for `token`, or `None` when unknown or revoked.
pub fn get(token: &str) -> Option<Grant> {
    if token.is_empty() {
        return None;
    }
    GRANTS
        .lock()
        .ok()
        .and_then(|g| g.as_ref().and_then(|m| m.get(token).cloned()))
}

/// Forget a grant. Handles already open on it keep reading — revocation stops
/// new `OPEN`s, it does not yank a descriptor out from under a read.
pub fn revoke(token: &str) {
    if let Ok(mut g) = GRANTS.lock() {
        if let Some(m) = g.as_mut() {
            m.remove(token);
        }
    }
}

/// The token in a peer-supplied path, if it addresses a grant at all.
pub fn token_of(path: &str) -> Option<&str> {
    path.strip_prefix(SHARE_PREFIX)
}

/// 16 bytes of OS randomness, hex. Unguessable on purpose: this token is the
/// only thing standing between a paired phone and a file the user did not
/// share with it, and the peer supplies it verbatim.
fn random_token() -> String {
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_get_revoke_round_trip() {
        let t = grant(PathBuf::from("/tmp/x"));
        assert_eq!(t.len(), 32);
        assert_eq!(get(&t).map(|g| g.path), Some(PathBuf::from("/tmp/x")));
        revoke(&t);
        assert!(get(&t).is_none());
        // Unknown and empty tokens resolve to nothing.
        assert!(get("").is_none());
        assert!(get("deadbeef").is_none());
    }

    #[test]
    fn tokens_are_distinct() {
        let a = grant(PathBuf::from("/a"));
        let b = grant(PathBuf::from("/a"));
        assert_ne!(a, b);
        revoke(&a);
        revoke(&b);
    }

    #[test]
    fn token_of_requires_the_prefix() {
        assert_eq!(token_of("share:abc"), Some("abc"));
        assert_eq!(token_of("/home/u/share:abc"), None);
    }
}
