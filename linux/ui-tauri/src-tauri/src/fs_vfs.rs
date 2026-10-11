//! The OS-independent half of presenting the phone's storage as a filesystem.
//!
//! Two mount adapters sit on top of this: [`crate::fs_mount`] (FUSE, Linux) and
//! [`crate::fs_projfs`] (ProjFS, Windows). They have almost nothing in common —
//! one answers a kernel protocol on a socket, the other implements COM-style
//! callbacks — but everything *between* them and the wire is the same work:
//!
//! * caching listings and attributes, so a file manager's stat storm does not
//!   become a round trip per file;
//! * turning an opaque peer address into a path and back;
//! * splitting a large read into protocol-sized ranged reads, pipelined;
//! * holding open handles, and capping how much of any of it is in flight.
//!
//! Keeping it here means the interesting bugs — the ones about identity,
//! staleness and reassembly — are written once and tested once, on whichever
//! machine happens to be running the tests. The adapters are left holding only
//! the part that genuinely differs.
//!
//! # Addresses are not paths
//!
//! The peer addresses an entry with an opaque token ([`p::FsEntry::path`]): an
//! absolute path on a real filesystem, but a `content://` document URI under
//! Android's SAF, where a name is simply not addressable. So a child's address
//! can only be *discovered*, by listing its parent — never constructed by
//! joining a name onto the parent's address. [`Vfs::resolve`] is that walk, and
//! it is why a mount adapter can hand us the `DCIM\Camera\foo.jpg` its OS gave
//! it without knowing what the phone will make of it.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vortex_l3_daemon::core::fs_proto::{self as p, code};

/// How long an attribute may be trusted.
///
/// The metadata cache design doc §7 asks for. Five seconds is long enough to
/// survive a file manager's stat storm and short enough that a file changed on
/// the phone shows up while the user is still looking at the folder.
///
/// On Linux this is also handed to the kernel, which then answers a repeat
/// `stat` without troubling this process at all.
pub(crate) const ATTR_TTL: Duration = Duration::from_secs(5);

/// How long a directory listing is kept.
///
/// Separate from [`ATTR_TTL`] because it serves a second purpose: a listing is
/// how a child's opaque address is discovered at all, so it is consulted on
/// paths no attribute cache would cover.
pub(crate) const DIR_TTL: Duration = Duration::from_secs(5);

/// Requests allowed on the link at once.
///
/// Not a throughput knob — a fairness one. A thumbnailer opening a folder of
/// photos issues as many reads as there are files, and an unbounded queue of
/// them would delay every listing behind megabytes of image data and, on a BLE
/// fallback, starve the session that carries everything else.
pub(crate) const MAX_INFLIGHT: usize = 8;

/// Ranged reads pipelined inside ONE filesystem read.
///
/// A caller asks for up to a few hundred KiB; the protocol caps a read at
/// 48 KiB. Those pieces go out together rather than one after another, for the
/// same reason [`crate::fs_link::read_all`] does it.
const READ_WINDOW: usize = 4;

/// Entries held for one directory. A guard against a peer that pages forever,
/// not a real limit — 200k files in one folder is already pathological.
const MAX_DIR_ENTRIES: usize = 200_000;

/// Cached directories, and cached attributes, before expired ones are swept.
///
/// Without a sweep a long browse would hold every listing it ever fetched for
/// the life of the process — which for this app is days. Sweeping on insert
/// past a threshold keeps it bounded without a timer, and the cost lands on the
/// operation that grew the map.
const CACHE_SWEEP_AT: usize = 4_096;

/// The address of the peer's synthetic root — the listing of what it shares.
pub(crate) const ROOT_ADDR: &str = "";

// ---------------------------------------------------------------------------
// The remote half
// ---------------------------------------------------------------------------

/// The peer-facing operations a mount needs.
///
/// Returns `impl Future + Send` rather than using `async fn` in the trait so
/// the futures can be `tokio::spawn`ed, which the FUSE adapter's concurrency
/// model depends on.
pub(crate) trait FsRemote: Send + Sync + 'static {
    fn list(
        &self,
        path: String,
        cursor: u32,
    ) -> impl Future<Output = Result<(Vec<p::FsEntry>, Option<u32>), i32>> + Send;
    fn stat(&self, path: String) -> impl Future<Output = Result<p::FsEntry, i32>> + Send;
    /// Returns `(handle, size at open time)`.
    fn open(&self, path: String) -> impl Future<Output = Result<(u64, u64), i32>> + Send;
    /// Returns `(bytes, eof)`. A short result is normal.
    fn read(
        &self,
        handle: u64,
        offset: u64,
        len: u32,
    ) -> impl Future<Output = Result<(Vec<u8>, bool), i32>> + Send;
    fn close(&self, handle: u64) -> impl Future<Output = ()> + Send;
}

/// The production remote: the protocol client over whatever transport is up.
pub(crate) struct LinkRemote;

// The trait declares `-> impl Future + Send`; an impl may satisfy that with a
// plain `async fn`, and the compiler still checks the future is `Send`.
impl FsRemote for LinkRemote {
    async fn list(
        &self,
        path: String,
        cursor: u32,
    ) -> Result<(Vec<p::FsEntry>, Option<u32>), i32> {
        crate::fs_link::list(&path, cursor).await
    }
    async fn stat(&self, path: String) -> Result<p::FsEntry, i32> {
        crate::fs_link::stat(&path).await
    }
    async fn open(&self, path: String) -> Result<(u64, u64), i32> {
        // Never for writing: the mount is read-only (design doc §8 step 5).
        crate::fs_link::open(&path, false).await
    }
    async fn read(&self, handle: u64, offset: u64, len: u32) -> Result<(Vec<u8>, bool), i32> {
        crate::fs_link::read(handle, offset, len).await
    }
    async fn close(&self, handle: u64) {
        crate::fs_link::close(handle).await
    }
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// A file a caller has open, keyed by the handle we handed back. Ours is a
/// separate number from the peer's, so a peer handle of 0 (or a reused one)
/// cannot collide with "no handle" — and so several of ours can share one of
/// the peer's.
struct OpenFile {
    addr: String,
}

/// The peer's handle on one file, shared by every local handle open on it.
///
/// Opening is where a file browser's per-file cost went. Gwenview, asked to
/// show one photo, reads the header of every file in the folder on its GUI
/// thread and opens each one three times in a row to do it: measured at 450
/// files in 35 s, ~53 ms a file, nearly all of it OPEN round trips. So:
///
/// * the peer is not asked at `open()` but at the first read — an open that is
///   never read costs nothing;
/// * overlapping opens of one file share one peer handle (the peer reads with
///   `pread`, so concurrent use of a handle is safe);
/// * the last close leaves it open for [`REMOTE_LINGER`], so open-close-open in
///   quick succession reuses it too.
struct SharedRemote {
    /// Local handles currently open on this file.
    users: usize,
    /// `(peer handle, size)`, filled by the first read. Left empty by a failed
    /// open so the next read retries rather than inheriting the failure.
    opened: Arc<tokio::sync::OnceCell<(u64, u64)>>,
    /// When `users` last fell to zero; `None` while in use.
    idle_since: Option<Instant>,
}

/// How long an unused peer handle is kept for a quick reopen. Long enough to
/// cover a program that reopens a file it just closed, short enough that a
/// file changed on the phone is not read through an old descriptor for long.
const REMOTE_LINGER: Duration = Duration::from_secs(2);

/// Idle peer handles kept at most. The phone refuses OPEN past 64 handles, and
/// a lingering handle should never be the reason a real open fails.
const MAX_IDLE_REMOTES: usize = 8;

/// A cached value and when it was cached, so a reader can check it against a
/// TTL without a second map.
type Cached<T> = (Instant, T);

/// The phone's filesystem, cached and addressed by opaque peer address.
pub(crate) struct Vfs<R: FsRemote> {
    pub(crate) remote: R,
    /// Listings by directory address. `Arc` so a hit does not copy a 10,000
    /// entry folder on its way out.
    dirs: Mutex<HashMap<String, Cached<Arc<Vec<p::FsEntry>>>>>,
    /// Attributes by address, seeded from listings.
    attrs: Mutex<HashMap<String, Cached<p::FsEntry>>>,
    files: Mutex<HashMap<u64, OpenFile>>,
    /// Peer handles by address; see [`SharedRemote`].
    remotes: Mutex<HashMap<String, SharedRemote>>,
    next_fh: AtomicU64,
    gate: tokio::sync::Semaphore,
    /// One lock per directory being fetched, so concurrent lookups in the same
    /// folder share a single listing instead of each pulling it whole. See
    /// [`Vfs::listing`].
    fetching: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl<R: FsRemote> Vfs<R> {
    pub(crate) fn new(remote: R) -> Self {
        Self {
            remote,
            dirs: Mutex::new(HashMap::new()),
            attrs: Mutex::new(HashMap::new()),
            files: Mutex::new(HashMap::new()),
            remotes: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
            gate: tokio::sync::Semaphore::new(MAX_INFLIGHT),
            fetching: Mutex::new(HashMap::new()),
        }
    }

    // Every lock below is a `std::sync::Mutex` held for a single map operation
    // and never across an `await`. Keeping that discipline is why the helpers
    // are this granular.

    /// Run one peer request under the concurrency cap.
    pub(crate) async fn gated<T>(&self, f: impl Future<Output = T>) -> T {
        // `acquire` only fails on a closed semaphore, and we never close it;
        // proceeding uncapped beats failing the operation.
        let _permit = self.gate.acquire().await;
        f.await
    }

    fn cached_dir(&self, addr: &str) -> Option<Arc<Vec<p::FsEntry>>> {
        let g = self.dirs.lock().ok()?;
        let (at, entries) = g.get(addr)?;
        (at.elapsed() < DIR_TTL).then(|| entries.clone())
    }

    fn cached_attr(&self, addr: &str) -> Option<p::FsEntry> {
        let g = self.attrs.lock().ok()?;
        let (at, entry) = g.get(addr)?;
        (at.elapsed() < ATTR_TTL).then(|| entry.clone())
    }

    fn store_attr(&self, entry: &p::FsEntry) {
        if let Ok(mut g) = self.attrs.lock() {
            if g.len() >= CACHE_SWEEP_AT {
                g.retain(|_, (at, _)| at.elapsed() < ATTR_TTL);
            }
            g.insert(entry.path.clone(), (Instant::now(), entry.clone()));
        }
    }

    /// A directory's entries, from cache or from the peer.
    ///
    /// Also where the attribute cache is seeded: a file manager follows a
    /// listing with a stat per entry, and answering those from the listing we
    /// already hold is the difference between one round trip per folder and one
    /// per file.
    pub(crate) async fn listing(&self, addr: &str) -> Result<Arc<Vec<p::FsEntry>>, i32> {
        if let Some(entries) = self.cached_dir(addr) {
            return Ok(entries);
        }
        // Single flight. A file manager generating thumbnails looks up every
        // file in a folder at once, and once the listing has expired each of
        // those lookups used to fetch the whole folder for itself — 485 entries
        // of DCIM/Camera, several pages each, a dozen times over, all competing
        // for the same [`MAX_INFLIGHT`] slots. Whoever gets here first fetches;
        // the rest wait on the same lock and find the result in the cache.
        let flight = self
            .fetching
            .lock()
            .map(|mut g| g.entry(addr.to_string()).or_default().clone())
            .unwrap_or_default();
        let _turn = flight.lock().await;
        if let Some(entries) = self.cached_dir(addr) {
            return Ok(entries);
        }
        let result = self.fetch_listing(addr).await;
        // Retire the lock once nobody else holds it; a waiter still queued on
        // it keeps its own clone and finds the cache filled.
        if let Ok(mut g) = self.fetching.lock() {
            if Arc::strong_count(&flight) <= 2 {
                g.remove(addr);
            }
        }
        result
    }

    /// The listing round trips themselves, under [`Vfs::listing`]'s lock.
    async fn fetch_listing(&self, addr: &str) -> Result<Arc<Vec<p::FsEntry>>, i32> {
        let mut all: Vec<p::FsEntry> = Vec::new();
        let mut cursor = 0u32;
        loop {
            let (page, next) = self.gated(self.remote.list(addr.to_string(), cursor)).await?;
            all.extend(page);
            match next {
                // A peer that keeps handing back the same cursor is not making
                // progress; stopping with a partial listing beats looping.
                Some(c) if c != cursor && all.len() < MAX_DIR_ENTRIES => cursor = c,
                _ => break,
            }
        }
        // An entry with no address cannot be opened or listed, so it would show
        // as a permanently broken row. Drop it and say so once.
        let before = all.len();
        all.retain(|e| !e.path.is_empty());
        if all.len() != before {
            tracing::warn!(
                dropped = before - all.len(),
                "fs-vfs: listing had entries with no address"
            );
        }
        for e in &all {
            self.store_attr(e);
        }
        let all = Arc::new(all);
        if let Ok(mut g) = self.dirs.lock() {
            if g.len() >= CACHE_SWEEP_AT {
                g.retain(|_, (at, _)| at.elapsed() < DIR_TTL);
            }
            g.insert(addr.to_string(), (Instant::now(), all.clone()));
        }
        Ok(all)
    }

    /// Resolve one name inside a directory.
    ///
    /// Through the parent's listing rather than by joining the name onto the
    /// parent's address, because the address is opaque — see the module docs.
    ///
    /// Exact match first, then case-insensitive. Windows callers arrive with
    /// whatever case the user typed and expect it to work; Android's storage is
    /// case-preserving but its FAT-derived emulation is not case-sensitive
    /// either, so a fold is the truthful behaviour on both sides. Exact still
    /// wins, so a peer that really does hold `README` and `readme` resolves
    /// each to itself.
    pub(crate) async fn lookup_child(&self, parent: &str, name: &str) -> Result<p::FsEntry, i32> {
        // An expired listing still says which address a name had. Checking that
        // one address with a STAT is a single small round trip; re-listing the
        // parent is the whole folder — measured at ~3 s for DCIM/Camera's 485
        // entries, paid by every lookup after the listing is 5 s old (an image
        // viewer opening a photo, a thumbnailer coming back to a file). Only a
        // name the old listing did not have, or one whose address is gone,
        // needs the full fetch.
        if self.cached_dir(parent).is_none() {
            let known = self
                .dirs
                .lock()
                .ok()
                .and_then(|g| g.get(parent).and_then(|(_, v)| find_child(v, name)));
            if let Some(known) = known {
                match self.entry_of(&known.path).await {
                    Ok(fresh) => return Ok(fresh),
                    // Deleted or renamed since: let the listing decide.
                    Err(c) if c == code::NOENT => {}
                    Err(c) => return Err(c),
                }
            }
        }
        let entries = self.listing(parent).await?;
        find_child(&entries, name).ok_or(code::NOENT)
    }

    /// One address's attributes.
    pub(crate) async fn entry_of(&self, addr: &str) -> Result<p::FsEntry, i32> {
        if addr == ROOT_ADDR {
            return Ok(root_entry());
        }
        if let Some(e) = self.cached_attr(addr) {
            return Ok(e);
        }
        let entry = self.gated(self.remote.stat(addr.to_string())).await?;
        self.store_attr(&entry);
        Ok(entry)
    }

    /// Resolve a path relative to the mount root.
    ///
    /// Used by the ProjFS adapter, which is addressed by path; FUSE is
    /// addressed by inode and resolves one component at a time through
    /// [`Vfs::lookup_child`]. Tested on every platform regardless — it is the
    /// hardest piece of the Windows adapter and the one least able to be
    /// tested there.
    ///
    /// Accepts either separator: an OS hands us its own, and a mount adapter
    /// should not have to translate before it can ask a question. Empty
    /// components are skipped, so a doubled separator or a trailing one is not
    /// an error.
    ///
    /// One listing per component, all of them cached, so walking a path the
    /// user is already browsing costs nothing.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) async fn resolve(&self, rel: &str) -> Result<p::FsEntry, i32> {
        let mut entry = root_entry();
        for part in rel.split(['\\', '/']).filter(|s| !s.is_empty()) {
            // `.` and `..` never reach us from either OS — both resolve them
            // before the request — and honouring `..` would need a parent link
            // an opaque address does not have. Refusing is the honest answer.
            if part == "." || part == ".." {
                return Err(code::INVAL);
            }
            if !entry.is_dir {
                // A path continues past a file: not "missing", but there is
                // nothing there to descend into.
                return Err(code::NOENT);
            }
            entry = self.lookup_child(&entry.path, part).await?;
        }
        Ok(entry)
    }

    /// Open a file. Does not ask the peer — see [`SharedRemote`] — so a file
    /// removed on the phone since it was listed fails on its first read rather
    /// than here.
    pub(crate) async fn open(&self, addr: &str) -> Result<u64, i32> {
        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut g) = self.remotes.lock() {
            let shared = g.entry(addr.to_string()).or_insert_with(|| SharedRemote {
                users: 0,
                opened: Default::default(),
                idle_since: None,
            });
            shared.users += 1;
            shared.idle_since = None;
        }
        if let Ok(mut g) = self.files.lock() {
            g.insert(
                fh,
                OpenFile {
                    addr: addr.to_string(),
                },
            );
        }
        Ok(fh)
    }

    /// The peer's `(handle, size)` for an open file, opening it on the peer if
    /// this is the first read through any handle on it.
    async fn remote_of(&self, fh: u64) -> Result<(u64, u64), i32> {
        let addr = self
            .files
            .lock()
            .ok()
            .and_then(|g| g.get(&fh).map(|f| f.addr.clone()))
            .ok_or(code::BADF)?;
        let opened = self
            .remotes
            .lock()
            .ok()
            .and_then(|g| g.get(&addr).map(|s| s.opened.clone()))
            .ok_or(code::BADF)?;
        opened
            .get_or_try_init(|| self.gated(self.remote.open(addr.clone())))
            .await
            .copied()
    }

    /// Release a handle. Unknown handles are ignored — a double close is the
    /// caller's business, not an error worth propagating.
    ///
    /// The peer handle outlives it briefly (see [`SharedRemote`]); this is also
    /// where lingering ones that have waited long enough are closed for real.
    pub(crate) async fn close(&self, fh: u64) {
        let Some(f) = self.files.lock().ok().and_then(|mut g| g.remove(&fh)) else {
            return;
        };
        let retired = self
            .remotes
            .lock()
            .map(|mut g| {
                if let Some(shared) = g.get_mut(&f.addr) {
                    shared.users = shared.users.saturating_sub(1);
                    if shared.users == 0 {
                        shared.idle_since = Some(Instant::now());
                    }
                }
                take_retired(&mut g)
            })
            .unwrap_or_default();
        for handle in retired {
            self.gated(self.remote.close(handle)).await;
        }
    }

    /// Read `size` bytes at `offset` from an open file, as one contiguous run.
    ///
    /// Splits into protocol-sized pieces and keeps [`READ_WINDOW`] of them in
    /// flight. `FuturesOrdered` yields in issue order, which is also offset
    /// order, so the pieces concatenate directly — and each future carries the
    /// offset it asked for, so a short piece is *detected* rather than silently
    /// shifting everything after it. On a gap we return the prefix: a read must
    /// be contiguous from `offset`, and a short reply is a legal answer.
    ///
    /// Clamped to the file's size at open, which saves a round trip for the
    /// page-sized overshoot a caller makes at the end of every file.
    pub(crate) async fn read_range(&self, fh: u64, offset: u64, size: u32) -> Result<Vec<u8>, i32> {
        use futures::stream::{FuturesOrdered, StreamExt};

        // Reads are clamped to the size the peer reported at open, so we never
        // ask for a range past the end just because the caller rounded up.
        let (remote, file_size) = self.remote_of(fh).await?;
        if offset >= file_size {
            return Ok(Vec::new());
        }
        let end = offset
            .saturating_add(size as u64)
            .min(file_size);
        let mut out: Vec<u8> = Vec::with_capacity((end - offset) as usize);
        let mut pending = FuturesOrdered::new();
        let mut next = offset;
        let mut expect = offset;
        loop {
            while pending.len() < READ_WINDOW && next < end {
                let at = next;
                let len = (end - at).min(p::MAX_READ_LEN as u64) as u32;
                pending.push_back(async move {
                    (at, len, self.gated(self.remote.read(remote, at, len)).await)
                });
                next = at + len as u64;
            }
            let Some((at, len, res)) = pending.next().await else {
                break;
            };
            let (bytes, mut eof) = res?;
            if at != expect {
                // Only reachable after the fill below gave up on a piece, so
                // this one starts past the end of what we have. Anything further
                // would land at the wrong file offset.
                break;
            }
            expect = at + bytes.len() as u64;
            out.extend_from_slice(&bytes);
            // A short piece is not the end of the file. The phone answers with
            // one `pread`, and Android's /sdcard is itself a FUSE mount that
            // may hand back less than asked. Passing that on would be worse
            // than slow: FUSE takes a short reply as "the file ends here", so a
            // photo came out cut off mid-file ("Corrupt JPEG data: premature
            // end of data segment") — and, with FOPEN_KEEP_CACHE, stayed that
            // way in the page cache. Ask for the rest of the piece instead.
            let piece_end = at + len as u64;
            while !eof && expect < piece_end {
                let want = (piece_end - expect) as u32;
                let (more, e) = self.gated(self.remote.read(remote, expect, want)).await?;
                if more.is_empty() {
                    // No progress and no EOF: the file shrank under us, or the
                    // peer is not making sense. Stop rather than spin.
                    eof = true;
                    break;
                }
                expect += more.len() as u64;
                out.extend_from_slice(&more);
                eof = e;
            }
            if eof || bytes.is_empty() {
                break;
            }
        }
        Ok(out)
    }
}

/// The mount root's own attributes.
///
/// Synthetic rather than a `STAT` of the empty address: the peer's root is a
/// list of what it shares, not a directory it can stat, and listing the mount
/// point must work regardless. A zero mtime rather than "now" so the root does
/// not appear to change on every remount.
/// Remove the idle peer handles that are due to close — past
/// [`REMOTE_LINGER`], or the oldest beyond [`MAX_IDLE_REMOTES`] — and return
/// the ones the peer actually holds.
fn take_retired(g: &mut HashMap<String, SharedRemote>) -> Vec<u64> {
    let mut idle: Vec<(Instant, String)> = g
        .iter()
        .filter_map(|(addr, s)| s.idle_since.map(|t| (t, addr.clone())))
        .collect();
    idle.sort();
    let over_cap = idle.len().saturating_sub(MAX_IDLE_REMOTES);
    let mut out = Vec::new();
    for (i, (since, addr)) in idle.into_iter().enumerate() {
        if i < over_cap || since.elapsed() >= REMOTE_LINGER {
            if let Some(&(handle, _)) = g.remove(&addr).as_ref().and_then(|s| s.opened.get()) {
                out.push(handle);
            }
        }
    }
    out
}

/// A name within a listing: exact match first, then case-insensitive (see
/// [`Vfs::lookup_child`] for why both).
fn find_child(entries: &[p::FsEntry], name: &str) -> Option<p::FsEntry> {
    entries
        .iter()
        .find(|e| e.name == name)
        .or_else(|| entries.iter().find(|e| e.name.eq_ignore_ascii_case(name)))
        .cloned()
}

pub(crate) fn root_entry() -> p::FsEntry {
    p::FsEntry {
        name: "/".to_string(),
        path: ROOT_ADDR.to_string(),
        is_dir: true,
        size: 0,
        mtime: 0,
        readonly: true,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A peer with a fixed tree, so the cache and the path walk can be tested
    /// without a phone, a kernel or a link. Counts requests: most of what this
    /// module does is avoid making them.
    pub(crate) struct FakePeer {
        /// address → entries, for directories.
        dirs: HashMap<String, Vec<p::FsEntry>>,
        /// address → contents, for files.
        files: HashMap<String, Vec<u8>>,
        pub(crate) lists: std::sync::atomic::AtomicU32,
        pub(crate) stats: std::sync::atomic::AtomicU32,
        pub(crate) reads: std::sync::atomic::AtomicU32,
        pub(crate) opens: std::sync::atomic::AtomicU32,
        pub(crate) closes: std::sync::atomic::AtomicU32,
        /// Answer every read with at most a third of what was asked, the way a
        /// single `pread` on Android's own FUSE-backed storage sometimes does.
        pub(crate) short_reads: std::sync::atomic::AtomicBool,
    }

    pub(crate) fn dir(name: &str, path: &str) -> p::FsEntry {
        p::FsEntry {
            name: name.into(),
            path: path.into(),
            is_dir: true,
            size: 0,
            mtime: 1_700_000_000,
            readonly: true,
        }
    }

    pub(crate) fn file(name: &str, path: &str, size: u64) -> p::FsEntry {
        p::FsEntry {
            name: name.into(),
            path: path.into(),
            is_dir: false,
            size,
            mtime: 1_700_000_000,
            readonly: true,
        }
    }

    /// The 100 KiB test file's contents — three protocol reads' worth.
    pub(crate) fn big_bytes() -> Vec<u8> {
        (0..100 * 1024).map(|i| (i % 251) as u8).collect()
    }

    impl FakePeer {
        pub(crate) fn new() -> Self {
            let big = big_bytes();
            let mut dirs = HashMap::new();
            dirs.insert(
                ROOT_ADDR.to_string(),
                vec![dir("DCIM", "/sdcard/DCIM"), file("a.txt", "/sdcard/a.txt", 5)],
            );
            dirs.insert(
                "/sdcard/DCIM".to_string(),
                vec![file("big.bin", "/sdcard/DCIM/big.bin", big.len() as u64)],
            );
            let mut files = HashMap::new();
            files.insert("/sdcard/a.txt".to_string(), b"hello".to_vec());
            files.insert("/sdcard/DCIM/big.bin".to_string(), big);
            // Readable but not listed: enough distinct files to overflow the
            // idle-handle cap.
            for i in 0..MAX_IDLE_REMOTES + 3 {
                files.insert(format!("/sdcard/many/{i}.bin"), vec![i as u8; 8]);
            }
            Self {
                dirs,
                files,
                lists: Default::default(),
                stats: Default::default(),
                reads: Default::default(),
                opens: Default::default(),
                closes: Default::default(),
                short_reads: Default::default(),
            }
        }
    }

    impl FsRemote for Arc<FakePeer> {
        fn list(
            &self,
            path: String,
            cursor: u32,
        ) -> impl Future<Output = Result<(Vec<p::FsEntry>, Option<u32>), i32>> + Send {
            let me = self.clone();
            async move {
                me.lists.fetch_add(1, Ordering::Relaxed);
                // A real round trip suspends; without this, concurrent callers
                // in a test would never actually overlap.
                tokio::task::yield_now().await;
                let all = me.dirs.get(&path).ok_or(code::NOENT)?;
                // One entry per page, so pagination is exercised rather than
                // assumed.
                let at = cursor as usize;
                match all.get(at) {
                    Some(e) => Ok((vec![e.clone()], (at + 1 < all.len()).then_some(cursor + 1))),
                    None => Ok((vec![], None)),
                }
            }
        }

        fn stat(&self, path: String) -> impl Future<Output = Result<p::FsEntry, i32>> + Send {
            let me = self.clone();
            async move {
                me.stats.fetch_add(1, Ordering::Relaxed);
                me.dirs
                    .values()
                    .flatten()
                    .find(|e| e.path == path)
                    .cloned()
                    .ok_or(code::NOENT)
            }
        }

        fn open(&self, path: String) -> impl Future<Output = Result<(u64, u64), i32>> + Send {
            let me = self.clone();
            async move {
                me.opens.fetch_add(1, Ordering::Relaxed);
                let bytes = me.files.get(&path).ok_or(code::NOENT)?;
                // The handle IS the address's index; enough to read it back.
                let idx = me.files.keys().position(|k| k == &path).unwrap() as u64;
                Ok((idx + 1, bytes.len() as u64))
            }
        }

        fn read(
            &self,
            handle: u64,
            offset: u64,
            len: u32,
        ) -> impl Future<Output = Result<(Vec<u8>, bool), i32>> + Send {
            let me = self.clone();
            async move {
                me.reads.fetch_add(1, Ordering::Relaxed);
                let key = me
                    .files
                    .keys()
                    .nth(handle as usize - 1)
                    .cloned()
                    .ok_or(code::BADF)?;
                let bytes = &me.files[&key];
                let at = (offset as usize).min(bytes.len());
                let len = if me.short_reads.load(Ordering::Relaxed) {
                    (len / 3).max(1)
                } else {
                    len
                };
                let to = (at + len as usize).min(bytes.len());
                Ok((bytes[at..to].to_vec(), to >= bytes.len()))
            }
        }

        async fn close(&self, _handle: u64) {
            self.closes.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn vfs() -> (Arc<Vfs<Arc<FakePeer>>>, Arc<FakePeer>) {
        let peer = Arc::new(FakePeer::new());
        (Arc::new(Vfs::new(peer.clone())), peer)
    }

    #[tokio::test]
    async fn listing_follows_pagination_to_the_end() {
        let (fs, peer) = vfs();
        let entries = fs.listing(ROOT_ADDR).await.unwrap();
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["DCIM", "a.txt"]
        );
        // Two pages plus the one that reports the end.
        assert_eq!(peer.lists.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn a_listing_answers_the_stats_that_follow_it() {
        let (fs, peer) = vfs();
        fs.listing(ROOT_ADDR).await.unwrap();
        let before = peer.lists.load(Ordering::Relaxed);
        let e = fs.lookup_child(ROOT_ADDR, "a.txt").await.unwrap();
        assert_eq!(e.path, "/sdcard/a.txt");
        // The point of the exercise: no further round trips, of any kind.
        assert_eq!(peer.lists.load(Ordering::Relaxed), before);
        assert_eq!(peer.stats.load(Ordering::Relaxed), 0);
        assert_eq!(fs.entry_of(&e.path).await.unwrap().name, "a.txt");
        assert_eq!(peer.stats.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn a_missing_name_is_noent_not_a_hang() {
        let (fs, _) = vfs();
        assert_eq!(
            fs.lookup_child(ROOT_ADDR, "nope").await.unwrap_err(),
            code::NOENT
        );
    }

    /// Age every cached listing and attribute past its TTL, as if the user
    /// had come back to the folder later.
    fn expire(fs: &Vfs<Arc<FakePeer>>) {
        let old = Instant::now()
            .checked_sub(DIR_TTL.max(ATTR_TTL) * 2)
            .expect("monotonic clock too young");
        for (at, _) in fs.dirs.lock().unwrap().values_mut() {
            *at = old;
        }
        for (at, _) in fs.attrs.lock().unwrap().values_mut() {
            *at = old;
        }
    }

    #[tokio::test]
    async fn concurrent_lookups_in_one_folder_share_a_single_listing() {
        let (fs, peer) = vfs();
        let lookups = (0..10).map(|_| fs.lookup_child(ROOT_ADDR, "a.txt"));
        for r in futures::future::join_all(lookups).await {
            assert_eq!(r.unwrap().path, "/sdcard/a.txt");
        }
        // One listing of two one-entry pages — not one per lookup. A thumbnail
        // storm in DCIM/Camera made this a dozen full fetches of 485 entries.
        assert_eq!(peer.lists.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn a_stale_lookup_revalidates_one_entry_instead_of_relisting() {
        let (fs, peer) = vfs();
        fs.listing(ROOT_ADDR).await.unwrap();
        expire(&fs);
        let lists = peer.lists.load(Ordering::Relaxed);
        let e = fs.lookup_child(ROOT_ADDR, "a.txt").await.unwrap();
        assert_eq!(e.path, "/sdcard/a.txt");
        assert_eq!(peer.lists.load(Ordering::Relaxed), lists, "re-listed the folder");
        assert_eq!(peer.stats.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn a_name_the_stale_listing_lacks_still_gets_a_fresh_listing() {
        let (fs, peer) = vfs();
        fs.listing(ROOT_ADDR).await.unwrap();
        expire(&fs);
        let lists = peer.lists.load(Ordering::Relaxed);
        // Could have appeared since; only the peer can say it did not.
        assert_eq!(fs.lookup_child(ROOT_ADDR, "new.jpg").await.unwrap_err(), code::NOENT);
        assert!(peer.lists.load(Ordering::Relaxed) > lists);
    }

    #[tokio::test]
    async fn the_root_stats_without_asking_the_peer() {
        let (fs, peer) = vfs();
        let e = fs.entry_of(ROOT_ADDR).await.unwrap();
        assert!(e.is_dir);
        assert_eq!(peer.stats.load(Ordering::Relaxed), 0);
    }

    // ── The path walk, which is what the ProjFS adapter stands on ──────────

    #[tokio::test]
    async fn a_path_resolves_to_the_peers_opaque_address() {
        let (fs, _) = vfs();
        // Both separators, because each OS hands us its own.
        for p in ["DCIM\\big.bin", "DCIM/big.bin"] {
            let e = fs.resolve(p).await.unwrap();
            assert_eq!(e.path, "/sdcard/DCIM/big.bin", "{p}");
            assert!(!e.is_dir);
        }
        assert_eq!(fs.resolve("DCIM").await.unwrap().path, "/sdcard/DCIM");
    }

    #[tokio::test]
    async fn the_empty_path_is_the_root() {
        let (fs, peer) = vfs();
        for p in ["", "\\", "/"] {
            assert_eq!(fs.resolve(p).await.unwrap().path, ROOT_ADDR, "{p}");
        }
        // The root is synthetic: resolving it must not ask the peer anything.
        assert_eq!(peer.lists.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn a_path_walk_is_case_insensitive_like_windows() {
        let (fs, _) = vfs();
        assert_eq!(
            fs.resolve("dcim\\BIG.BIN").await.unwrap().path,
            "/sdcard/DCIM/big.bin"
        );
    }

    #[tokio::test]
    async fn descending_into_a_file_is_not_found() {
        let (fs, _) = vfs();
        assert_eq!(
            fs.resolve("a.txt\\nope").await.unwrap_err(),
            code::NOENT,
            "a path may not continue past a file"
        );
    }

    #[tokio::test]
    async fn dot_and_dotdot_are_refused_rather_than_guessed() {
        let (fs, _) = vfs();
        // Neither OS sends these, and an opaque address has no parent link, so
        // answering would mean inventing one.
        assert_eq!(fs.resolve("DCIM\\..").await.unwrap_err(), code::INVAL);
        assert_eq!(fs.resolve(".").await.unwrap_err(), code::INVAL);
    }

    #[tokio::test]
    async fn a_missing_component_is_noent() {
        let (fs, _) = vfs();
        assert_eq!(fs.resolve("DCIM\\nope.bin").await.unwrap_err(), code::NOENT);
        assert_eq!(fs.resolve("nope\\deep").await.unwrap_err(), code::NOENT);
    }

    // ── Reads ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_read_larger_than_the_protocol_limit_is_split_and_reassembled() {
        let (fs, peer) = vfs();
        let fh = fs.open("/sdcard/DCIM/big.bin").await.unwrap();
        let bytes = fs.read_range(fh, 0, 100 * 1024).await.unwrap();
        assert_eq!(bytes, big_bytes(), "reassembled in the wrong order");
        // 48 + 48 + 4 KiB.
        assert_eq!(peer.reads.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn a_short_answer_is_completed_not_passed_on_as_eof() {
        let (fs, peer) = vfs();
        peer.short_reads.store(true, Ordering::Relaxed);
        let fh = fs.open("/sdcard/DCIM/big.bin").await.unwrap();
        // From the start and from an offset, so both the first piece and a
        // later one exercise the fill.
        assert_eq!(fs.read_range(fh, 0, 100 * 1024).await.unwrap(), big_bytes());
        let at = 30 * 1024;
        assert_eq!(
            fs.read_range(fh, at, 64 * 1024).await.unwrap(),
            big_bytes()[at as usize..at as usize + 64 * 1024]
        );
    }

    #[tokio::test]
    async fn a_read_at_an_offset_starts_there() {
        let (fs, _) = vfs();
        let fh = fs.open("/sdcard/DCIM/big.bin").await.unwrap();
        let at = 70 * 1024u64;
        let bytes = fs.read_range(fh, at, 100 * 1024).await.unwrap();
        assert_eq!(bytes, big_bytes()[at as usize..]);
    }

    #[tokio::test]
    async fn a_read_is_clamped_to_the_file_rather_than_asking_past_it() {
        let (fs, peer) = vfs();
        let fh = fs.open("/sdcard/a.txt").await.unwrap();
        // A caller asking for a whole page of a 5-byte file is the normal case.
        assert_eq!(fs.read_range(fh, 0, 4096).await.unwrap(), b"hello");
        assert_eq!(peer.reads.load(Ordering::Relaxed), 1);
        // Past the end costs no round trip at all.
        assert!(fs.read_range(fh, 5, 4096).await.unwrap().is_empty());
        assert_eq!(peer.reads.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn a_closed_handle_is_bad_not_a_panic() {
        let (fs, _) = vfs();
        let fh = fs.open("/sdcard/a.txt").await.unwrap();
        fs.close(fh).await;
        assert_eq!(fs.read_range(fh, 0, 16).await.unwrap_err(), code::BADF);
        // A second close is the caller's business, not an error.
        fs.close(fh).await;
    }

    // ── Peer handles ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn reopening_a_file_reuses_one_peer_handle() {
        let (fs, peer) = vfs();
        // Gwenview's pattern: the same file opened three times in a row, and
        // two of those handles open at once.
        let a = fs.open("/sdcard/a.txt").await.unwrap();
        let b = fs.open("/sdcard/a.txt").await.unwrap();
        assert_eq!(fs.read_range(a, 0, 5).await.unwrap(), b"hello");
        assert_eq!(fs.read_range(b, 0, 5).await.unwrap(), b"hello");
        fs.close(a).await;
        fs.close(b).await;
        let c = fs.open("/sdcard/a.txt").await.unwrap();
        assert_eq!(fs.read_range(c, 0, 5).await.unwrap(), b"hello");
        fs.close(c).await;
        assert_eq!(peer.opens.load(Ordering::Relaxed), 1, "one OPEN for all three");
        assert_eq!(peer.closes.load(Ordering::Relaxed), 0, "still lingering");
    }

    #[tokio::test]
    async fn an_open_that_is_never_read_costs_no_round_trip() {
        let (fs, peer) = vfs();
        let fh = fs.open("/sdcard/a.txt").await.unwrap();
        fs.close(fh).await;
        assert_eq!(peer.opens.load(Ordering::Relaxed), 0);
        assert_eq!(peer.closes.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn a_file_gone_from_the_phone_fails_on_its_first_read() {
        let (fs, _) = vfs();
        let fh = fs.open("/sdcard/gone.jpg").await.unwrap();
        assert_eq!(fs.read_range(fh, 0, 16).await.unwrap_err(), code::NOENT);
        // And keeps failing rather than caching the failure as a handle.
        assert_eq!(fs.read_range(fh, 0, 16).await.unwrap_err(), code::NOENT);
        fs.close(fh).await;
    }

    #[tokio::test]
    async fn idle_peer_handles_are_capped() {
        let (fs, peer) = vfs();
        let n = MAX_IDLE_REMOTES + 3;
        for i in 0..n {
            let fh = fs.open(&format!("/sdcard/many/{i}.bin")).await.unwrap();
            assert_eq!(fs.read_range(fh, 0, 8).await.unwrap(), vec![i as u8; 8]);
            fs.close(fh).await;
        }
        assert_eq!(peer.opens.load(Ordering::Relaxed), n as u32);
        // The phone holds 64 handles at most; lingering ones must not be the
        // reason a real OPEN is refused.
        assert_eq!(peer.closes.load(Ordering::Relaxed), 3);
    }
}
