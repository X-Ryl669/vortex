//! Where each capture the phone sent us was saved, and a trash to move it to
//! when the phone says the original is gone.
//!
//! Deleting a picture on the phone should not leave its twin sitting on the
//! laptop for ever — that is the whole point of the copy being automatic. To
//! act on that we need two things the transfer itself does not keep: which
//! local file a phone-side token became, and somewhere to put it that is not
//! "gone".
//!
//! **Only captures.** Nothing here touches a file the user shared by hand, or
//! anything outside the folder a capture was written to. Vortex deletes only
//! what Vortex wrote.
//!
//! **Never a hard delete.** A removal that travelled over a network on the
//! strength of a hash is exactly the kind of thing that should be undoable, so
//! the file moves to [`trash_dir`] and is purged after [`TRASH_KEEP_DAYS`] —
//! the same bargain every desktop trash makes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How long a trashed capture stays recoverable.
const TRASH_KEEP_DAYS: u64 = 30;

/// Most captures to remember. Each entry is a path and a timestamp; this is
/// months of ordinary use, and the oldest fall off first.
const MAX_ENTRIES: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// Where the capture was written.
    path: PathBuf,
    /// When, in unix seconds — the eviction order, and what the file name
    /// under the trash is stamped with.
    at: u64,
}

/// token → where it landed. Ordered so eviction has a defined victim.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Ledger {
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
}

fn state() -> &'static Mutex<Option<Ledger>> {
    static STATE: Mutex<Option<Ledger>> = Mutex::new(None);
    &STATE
}

fn ledger_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/vortex/captures.json"))
}

/// Where a capture goes when the phone says its original is gone.
fn trash_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/vortex/trash"))
}

fn now_sec() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Run `f` against the ledger, loading it on first use and writing it back if
/// `f` changed anything. A ledger that cannot be read starts empty: losing the
/// record means a deletion is not mirrored, which is the harmless direction.
fn with_ledger<T>(f: impl FnOnce(&mut Ledger) -> (T, bool)) -> T {
    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let loaded = ledger_path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Ledger>(&b).ok())
            .unwrap_or_default();
        *guard = Some(loaded);
    }
    let ledger = guard.as_mut().expect("loaded above");
    let (out, dirty) = f(ledger);
    if dirty {
        // Evict oldest-first so the map cannot grow without bound.
        while ledger.entries.len() > MAX_ENTRIES {
            let oldest = ledger
                .entries
                .iter()
                .min_by_key(|(_, e)| e.at)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    ledger.entries.remove(&k);
                }
                None => break,
            }
        }
        if let Some(p) = ledger_path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_vec_pretty(ledger) {
                let _ = vortex_l3_daemon::core::fs_private::write_private(&p, &json);
            }
        }
    }
    out
}

/// Remember that `token`'s bytes were written to `path`.
pub(crate) fn record(token: &str, path: &Path) {
    if token.is_empty() {
        return;
    }
    with_ledger(|l| {
        l.entries.insert(
            token.to_string(),
            Entry { path: path.to_path_buf(), at: now_sec() },
        );
        ((), true)
    });
}

/// The phone says the original behind `token` is gone. Move our copy to the
/// trash and report where it went, or `None` when there is nothing to move —
/// an unknown token, or a file the user already removed themselves.
pub(crate) fn trash_for_token(token: &str) -> Option<PathBuf> {
    let entry = with_ledger(|l| {
        let e = l.entries.remove(token);
        let dirty = e.is_some();
        (e, dirty)
    })?;
    if !entry.path.exists() {
        tracing::info!(token, "phone deleted its original; our copy was already gone");
        return None;
    }
    let dir = trash_dir()?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("capture trash unavailable ({e}); leaving the file in place");
        return None;
    }
    let name = entry.path.file_name().map(|n| n.to_string_lossy().to_string())?;
    // Stamped, so two captures with the same name cannot collide in the trash.
    let target = dir.join(format!("{}-{name}", now_sec()));
    match std::fs::rename(&entry.path, &target) {
        Ok(()) => {
            tracing::info!(
                from = %entry.path.display(),
                "phone deleted its original; our copy moved to the trash"
            );
            Some(target)
        }
        Err(e) => {
            // Across filesystems rename fails; a copy+remove is the fallback,
            // and if THAT fails the file simply stays where it is.
            match std::fs::copy(&entry.path, &target).and_then(|_| std::fs::remove_file(&entry.path))
            {
                Ok(()) => Some(target),
                Err(_) => {
                    tracing::warn!("could not trash {}: {e}", entry.path.display());
                    None
                }
            }
        }
    }
}

/// When we trashed the file called `name`, from the stamp `trash_for_token`
/// puts in front of it.
///
/// Judged by the stamp rather than by an mtime, because a copy carries the
/// ORIGINAL's mtime — a photo taken last year would be purged the moment it
/// reached the trash, which is the one outcome a trash exists to prevent.
///
/// Anything without a leading number is not ours to delete. The purge walks a
/// directory and removes files from it, so a name it cannot account for must
/// be left alone rather than guessed at.
fn stamp_of(name: &str) -> Option<u64> {
    let digits = name.split('-').next()?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// Drop trashed captures older than [`TRASH_KEEP_DAYS`]. Cheap, and safe to
/// call on a timer: it only ever looks inside our own trash directory.
pub(crate) fn purge_trash() {
    let Some(dir) = trash_dir() else { return };
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let cutoff = now_sec().saturating_sub(TRASH_KEEP_DAYS * 24 * 60 * 60);
    let mut purged = 0usize;
    for e in rd.flatten() {
        let Some(stamp) = stamp_of(&e.file_name().to_string_lossy()) else { continue };
        if stamp < cutoff && std::fs::remove_file(e.path()).is_ok() {
            purged += 1;
        }
    }
    if purged > 0 {
        tracing::info!(purged, days = TRASH_KEEP_DAYS, "purged old trashed captures");
    }
}

#[cfg(test)]
mod tests {
    use super::stamp_of;

    /// The name `trash_for_token` writes: our stamp, then the original name.
    #[test]
    fn the_stamp_is_read_back_off_the_name() {
        assert_eq!(stamp_of("1789200000-Screenshot_2026-09-12.jpg"), Some(1789200000));
        // The original name having dashes of its own changes nothing: only the
        // first segment is ours.
        assert_eq!(stamp_of("42-a-b-c.png"), Some(42));
    }

    /// The purge deletes what this function accepts, so anything it cannot
    /// account for has to come back None — a file dropped in the trash by hand
    /// is not ours to remove on a thirty-day timer.
    #[test]
    fn a_name_without_our_stamp_is_left_alone() {
        assert_eq!(stamp_of("holiday.jpg"), None);
        assert_eq!(stamp_of("-leading-dash.jpg"), None);
        assert_eq!(stamp_of("12ab-mixed.jpg"), None);
        assert_eq!(stamp_of(""), None);
    }
}
