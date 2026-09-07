//! The phone's storage as a real filesystem, via FUSE (design doc §8 step 6).
//!
//! Dolphin, Nautilus, `cp`, mpv and every thumbnailer already know how to talk
//! to a filesystem, so the cheapest way to make the phone's files usable is to
//! be one. This module is the Linux **mount adapter**: it turns kernel FUSE
//! operations into the ranged-filesystem protocol and back. The phone is
//! untouched by it — that is the whole point of §2's "the phone serves a dumb,
//! narrow protocol; the laptop does everything clever".
//!
//! # Why FUSE and not the WebDAV gateway first
//!
//! The doc sequenced WebDAV ahead of this because one gateway serves both
//! operating systems. On Linux it buys nothing FUSE does not: GVFS/KIO mount
//! `davs://` in *their* process, so only their own file dialogs see the files —
//! `cp`, `mpv` and every non-KIO program do not. Windows' WebClient also caps a
//! file at ~50 MB, and escaping a 64 MB cap into a 50 MB one would be absurd.
//! A FUSE mount is a real path in the filesystem with no ceiling, and ProjFS
//! gives Windows the same later.
//!
//! # Concurrency, which is the load-bearing design decision
//!
//! FUSE hands us one request at a time on one thread. Answering each one
//! inline — issue the request, block on the phone's reply, return — would make
//! the mount as slow as the round trip *times* the number of operations, and a
//! file manager stats every visible file at once. So every operation is
//! immediately handed to the async runtime and its `Reply` object (which is
//! `Send`, deliberately) is answered from there. The session thread does
//! nothing but parse and dispatch.
//!
//! That is also what makes the kernel's own readahead work for us: a sequential
//! reader triggers several `read` calls at once, and because we never block,
//! they overlap on the wire instead of queueing.
//!
//! Two brakes on it: a semaphore caps how many requests may be on the link at
//! once (a thumbnailer will otherwise fire dozens and starve the BLE session
//! with them), and each `read` splits into at most [`READ_WINDOW`] pipelined
//! ranged reads.
//!
//! # Why a `FsRemote` trait
//!
//! The interesting bugs here are ours — inode identity, cache staleness,
//! reassembling a short read — and none of them need a phone to reproduce. The
//! trait lets the tests below drive the whole filesystem against a fake tree in
//! memory; [`LinkRemote`] is the one-line production implementation.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use fuser::{
    Errno, FileAttr, FileType, FopenFlags, Generation, INodeNo, KernelConfig, MountOption,
    OpenAccMode, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, Request,
};
use vortex_l3_daemon::core::fs_proto::{self as p, code};

/// How long the kernel may trust an attribute or a directory entry.
///
/// This is the metadata cache design doc §7 asks for, and the kernel gives it
/// to us for free: within the TTL a `stat` never reaches this process, let
/// alone the phone. Five seconds is long enough to survive a file manager's
/// stat storm and short enough that a file changed on the phone shows up while
/// the user is still looking at the folder.
const ATTR_TTL: Duration = Duration::from_secs(5);

/// How long *we* keep a directory listing.
///
/// Separate from [`ATTR_TTL`] because it serves a different purpose: a listing
/// is how a child's opaque address is discovered at all (see [`Inner::listing`]),
/// so it is consulted on paths the kernel cache never reaches.
const DIR_TTL: Duration = Duration::from_secs(5);

/// Requests allowed on the link at once.
///
/// Not a throughput knob — a fairness one. A thumbnailer opening a folder of
/// photos will issue as many reads as there are files, and an unbounded queue
/// of them would delay every listing behind megabytes of image data and, on a
/// BLE fallback, starve the session that carries everything else.
const MAX_INFLIGHT: usize = 8;

/// Ranged reads pipelined inside ONE FUSE read.
///
/// The kernel asks for up to 128 KiB; the protocol caps a read at 48 KiB. Those
/// pieces are issued together rather than one after another, for the same
/// reason [`crate::fs_link::read_all`] does it.
const READ_WINDOW: usize = 4;

/// The mount root. FUSE fixes this at 1; the phone's synthetic root (`""`, the
/// path that lists what it shares) lives here.
const ROOT_INO: u64 = 1;

/// Entries we will hold for one directory. A guard against a peer that pages
/// forever, not a real limit — 200k files in one folder is already pathological.
const MAX_DIR_ENTRIES: usize = 200_000;

/// Cached directories, and cached attributes, before expired ones are swept.
///
/// The caches are keyed by inode and inodes are never recycled, so without a
/// sweep a long browse would hold every listing it ever fetched for the life of
/// the process — which for this app is days. Sweeping on insert past a
/// threshold keeps it bounded without a timer, and the cost lands on the
/// operation that grew the map.
const CACHE_SWEEP_AT: usize = 4_096;

// ---------------------------------------------------------------------------
// The remote half
// ---------------------------------------------------------------------------

/// The peer-facing operations the mount needs.
///
/// Returns `impl Future + Send` rather than using `async fn` in the trait so
/// the futures can be `tokio::spawn`ed, which is the entire concurrency model
/// above.
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
// Inode identity
// ---------------------------------------------------------------------------

/// Inode numbers ↔ peer addresses.
///
/// A peer address is an opaque token, not a path — on Android it is a document
/// URI — so an inode number cannot be derived from one and the mapping has to
/// be remembered. Numbers are **never recycled**: a file manager holds inode
/// numbers across a refresh and reusing one would silently show it the wrong
/// file. The table therefore only grows, which is fine at a few dozen bytes per
/// entry for a session's worth of browsing.
#[derive(Default)]
struct Inodes {
    by_ino: HashMap<u64, String>,
    by_path: HashMap<String, u64>,
    next: u64,
}

impl Inodes {
    fn new() -> Self {
        let mut t = Self {
            next: ROOT_INO + 1,
            ..Default::default()
        };
        // The peer's synthetic root: the empty path is what lists its shares.
        t.by_ino.insert(ROOT_INO, String::new());
        t.by_path.insert(String::new(), ROOT_INO);
        t
    }

    fn intern(&mut self, path: &str) -> u64 {
        if let Some(&ino) = self.by_path.get(path) {
            return ino;
        }
        let ino = self.next;
        self.next += 1;
        self.by_ino.insert(ino, path.to_string());
        self.by_path.insert(path.to_string(), ino);
        ino
    }

    fn path(&self, ino: u64) -> Option<&str> {
        self.by_ino.get(&ino).map(String::as_str)
    }
}

/// A file the kernel has open, keyed by the handle we handed back from `open`.
struct OpenFile {
    /// The peer's handle. Ours is a separate number so a peer handle of 0 (or a
    /// reused one) cannot collide with "no handle".
    remote: u64,
    /// Size as of `open`. Reads are clamped to it so we never ask the phone for
    /// a range past the end just because the kernel rounded up to a page.
    size: u64,
}

struct Inner<R: FsRemote> {
    remote: R,
    inodes: Mutex<Inodes>,
    /// Listings by directory inode.
    dirs: Mutex<HashMap<u64, (Instant, Vec<p::FsEntry>)>>,
    /// Attributes by inode, seeded from listings.
    attrs: Mutex<HashMap<u64, (Instant, p::FsEntry)>>,
    files: Mutex<HashMap<u64, OpenFile>>,
    next_fh: AtomicU64,
    gate: tokio::sync::Semaphore,
}

impl<R: FsRemote> Inner<R> {
    fn new(remote: R) -> Self {
        Self {
            remote,
            inodes: Mutex::new(Inodes::new()),
            dirs: Mutex::new(HashMap::new()),
            attrs: Mutex::new(HashMap::new()),
            files: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
            gate: tokio::sync::Semaphore::new(MAX_INFLIGHT),
        }
    }

    // Every lock here is a `std::sync::Mutex` held for a single map operation
    // and never across an `await`. Keeping that discipline is why the helpers
    // are this granular.

    fn intern(&self, path: &str) -> u64 {
        self.inodes
            .lock()
            .map(|mut t| t.intern(path))
            .unwrap_or(ROOT_INO)
    }

    fn path_of(&self, ino: u64) -> Option<String> {
        self.inodes
            .lock()
            .ok()
            .and_then(|t| t.path(ino).map(str::to_string))
    }

    fn cached_dir(&self, ino: u64) -> Option<Vec<p::FsEntry>> {
        let g = self.dirs.lock().ok()?;
        let (at, entries) = g.get(&ino)?;
        (at.elapsed() < DIR_TTL).then(|| entries.clone())
    }

    fn cached_attr(&self, ino: u64) -> Option<p::FsEntry> {
        let g = self.attrs.lock().ok()?;
        let (at, entry) = g.get(&ino)?;
        (at.elapsed() < ATTR_TTL).then(|| entry.clone())
    }

    fn store_attr(&self, ino: u64, entry: &p::FsEntry) {
        if let Ok(mut g) = self.attrs.lock() {
            if g.len() >= CACHE_SWEEP_AT {
                g.retain(|_, (at, _)| at.elapsed() < ATTR_TTL);
            }
            g.insert(ino, (Instant::now(), entry.clone()));
        }
    }

    /// Run one peer request under the concurrency cap.
    async fn gated<T>(&self, f: impl Future<Output = T>) -> T {
        // `acquire` only fails on a closed semaphore, and we never close it;
        // proceeding uncapped beats failing the operation.
        let _permit = self.gate.acquire().await;
        f.await
    }

    /// A directory's entries, from cache or from the peer.
    ///
    /// Also where child inodes are minted and the attribute cache is seeded:
    /// a file manager follows every `readdir` with a `lookup` and a `getattr`
    /// per entry, and answering those from the listing we already have is the
    /// difference between one round trip per folder and one per file.
    async fn listing(&self, ino: u64) -> Result<Vec<p::FsEntry>, i32> {
        if let Some(entries) = self.cached_dir(ino) {
            return Ok(entries);
        }
        let path = self.path_of(ino).ok_or(code::NOENT)?;
        let mut all: Vec<p::FsEntry> = Vec::new();
        let mut cursor = 0u32;
        loop {
            let (page, next) = self.gated(self.remote.list(path.clone(), cursor)).await?;
            all.extend(page);
            match next {
                // A peer that keeps handing back the same cursor is not making
                // progress; stopping with a partial listing beats looping.
                Some(c) if c != cursor && all.len() < MAX_DIR_ENTRIES => cursor = c,
                _ => break,
            }
        }
        // An entry with no address cannot be opened or listed, so it would
        // appear as a permanently broken row. Drop it and say so once.
        let before = all.len();
        all.retain(|e| !e.path.is_empty());
        if all.len() != before {
            tracing::warn!(
                dropped = before - all.len(),
                "fs-mount: listing had entries with no address"
            );
        }
        for e in &all {
            let child = self.intern(&e.path);
            self.store_attr(child, e);
        }
        if let Ok(mut g) = self.dirs.lock() {
            if g.len() >= CACHE_SWEEP_AT {
                g.retain(|_, (at, _)| at.elapsed() < DIR_TTL);
            }
            g.insert(ino, (Instant::now(), all.clone()));
        }
        Ok(all)
    }

    /// Resolve one name inside a directory.
    ///
    /// Goes through the parent's listing rather than joining the name onto the
    /// parent's path, because a child's address is opaque: under SAF a name is
    /// simply not addressable, and constructing `parent/name` would produce a
    /// path the phone cannot resolve.
    async fn lookup_child(&self, parent: u64, name: &str) -> Result<(u64, p::FsEntry), i32> {
        let entries = self.listing(parent).await?;
        let entry = entries
            .into_iter()
            .find(|e| e.name == name)
            .ok_or(code::NOENT)?;
        let ino = self.intern(&entry.path);
        self.store_attr(ino, &entry);
        Ok((ino, entry))
    }

    /// One inode's attributes.
    async fn entry_of(&self, ino: u64) -> Result<p::FsEntry, i32> {
        if ino == ROOT_INO {
            return Ok(root_entry());
        }
        if let Some(e) = self.cached_attr(ino) {
            return Ok(e);
        }
        let path = self.path_of(ino).ok_or(code::NOENT)?;
        let entry = self.gated(self.remote.stat(path)).await?;
        self.store_attr(ino, &entry);
        Ok(entry)
    }

    /// Read `size` bytes at `offset` from an open file, as one contiguous run.
    ///
    /// Splits into protocol-sized pieces and keeps [`READ_WINDOW`] of them in
    /// flight. `FuturesOrdered` yields in issue order, which is also offset
    /// order, so the pieces concatenate directly — and each future carries the
    /// offset it asked for, so a short piece is *detected* rather than silently
    /// shifting everything after it. On a gap we return the prefix: a FUSE read
    /// must be contiguous from `offset`, and a short reply is a legal answer.
    async fn read_range(&self, remote: u64, offset: u64, size: u32) -> Result<Vec<u8>, i32> {
        use futures::stream::{FuturesOrdered, StreamExt};

        let end = offset.saturating_add(size as u64);
        let mut out: Vec<u8> = Vec::new();
        let mut pending = FuturesOrdered::new();
        let mut next = offset;
        let mut expect = offset;
        loop {
            while pending.len() < READ_WINDOW && next < end {
                let at = next;
                let len = (end - at).min(p::MAX_READ_LEN as u64) as u32;
                pending.push_back(async move {
                    (at, self.gated(self.remote.read(remote, at, len)).await)
                });
                next = at + len as u64;
            }
            let Some((at, res)) = pending.next().await else {
                break;
            };
            let (bytes, eof) = res?;
            if at != expect {
                // An earlier piece came back short, so this one starts past the
                // end of what we have. Anything further would land at the wrong
                // file offset.
                break;
            }
            expect = at + bytes.len() as u64;
            out.extend_from_slice(&bytes);
            if eof || bytes.is_empty() {
                break;
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

/// The mount root's own attributes.
///
/// Synthetic rather than a `STAT` of the empty path: the peer's root is a list
/// of what it shares, not a directory it can stat, and `ls` of the mount point
/// must work regardless. `UNIX_EPOCH` rather than "now" so the kernel does not
/// see the root's mtime change on every remount.
fn root_entry() -> p::FsEntry {
    p::FsEntry {
        name: "/".to_string(),
        path: String::new(),
        is_dir: true,
        size: 0,
        mtime: 0,
        readonly: true,
    }
}

/// A protocol entry as a kernel `stat`.
///
/// Permissions are fixed rather than reported by the peer: the mount is
/// read-only until design doc §8 step 5 lands, and a writable-looking mode bit
/// would only get a copy half-way through before the phone refused it. `nlink`
/// of 2 for a directory is the usual lie (`.` and `..`) — the real subdirectory
/// count would cost a listing per stat.
fn attr_of(ino: u64, e: &p::FsEntry, uid: u32, gid: u32) -> FileAttr {
    let mtime = mtime_of(e.mtime);
    FileAttr {
        ino: INodeNo(ino),
        size: if e.is_dir { 0 } else { e.size },
        blocks: e.size.div_ceil(512),
        atime: mtime,
        mtime,
        ctime: mtime,
        crtime: mtime,
        kind: if e.is_dir {
            FileType::Directory
        } else {
            FileType::RegularFile
        },
        perm: if e.is_dir { 0o555 } else { 0o444 },
        nlink: if e.is_dir { 2 } else { 1 },
        uid,
        gid,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

/// Seconds since the epoch as a `SystemTime`, tolerating the 0 the protocol
/// uses for "the peer cannot tell" and the negative values a badly-set phone
/// clock can produce.
fn mtime_of(secs: i64) -> SystemTime {
    if secs >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs as u64)
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs())
    }
}

/// A protocol code as an errno.
///
/// The reason [`code`] is errno-shaped in the first place: this is meant to be
/// a rename, not a translation. What the file manager shows the user comes
/// straight from here, so [`crate::fs_link::NO_LINK`] mapping to `EHOSTDOWN`
/// ("Host is down") rather than a generic I/O error is the difference between
/// an accurate message and a puzzling one.
fn errno_of(c: i32) -> Errno {
    match c {
        code::NOENT => Errno::ENOENT,
        code::ACCES => Errno::EACCES,
        code::BADF => Errno::EBADF,
        code::INVAL => Errno::EINVAL,
        code::NOTSUP => Errno::ENOTSUP,
        code::ISDIR => Errno::EISDIR,
        code::ROFS => Errno::EROFS,
        crate::fs_link::NO_LINK => Errno::EHOSTDOWN,
        // Includes `code::IO`, and anything a future peer invents.
        _ => Errno::EIO,
    }
}

// ---------------------------------------------------------------------------
// The filesystem
// ---------------------------------------------------------------------------

struct PhoneFs<R: FsRemote> {
    inner: Arc<Inner<R>>,
    rt: tokio::runtime::Handle,
}

/// Hand `body` the runtime and let it answer whenever the phone does.
///
/// Every operation goes through here, which is what keeps the FUSE session
/// thread free to dispatch the next one. Nothing waits on the result: the
/// `Reply` carries the request id, so the answer finds its way back on its own.
macro_rules! detach {
    ($fs:expr, |$inner:ident| $body:block) => {{
        let $inner = $fs.inner.clone();
        $fs.rt.spawn(async move { $body });
    }};
}

impl<R: FsRemote> fuser::Filesystem for PhoneFs<R> {
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> std::io::Result<()> {
        // Readahead is the one §7 item the kernel implements for us: it turns a
        // sequential reader into several overlapping `read` calls, and because
        // we never block one, they overlap on the wire too. Ask for as much as
        // it will give (it clamps and reports what it took).
        let readahead = config.set_max_readahead(1024 * 1024).unwrap_or_else(|max| {
            let _ = config.set_max_readahead(max);
            max
        });
        // Background requests are how many of those may be outstanding. Ours
        // are answered off-thread, so a deeper queue costs nothing here.
        let _ = config.set_max_background(MAX_INFLIGHT as u16 * 2);
        tracing::info!(readahead, "fs-mount: kernel session up");
        Ok(())
    }

    fn lookup(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        // Names arrive from the kernel as bytes and reach us over JSON, so a
        // name that is not UTF-8 cannot round-trip in the first place; matching
        // lossily just makes it miss rather than panic.
        let name = name.to_string_lossy().to_string();
        let (uid, gid) = (req.uid(), req.gid());
        let parent = parent.0;
        detach!(self, |inner| {
            match inner.lookup_child(parent, &name).await {
                Ok((ino, e)) => reply.entry(&ATTR_TTL, &attr_of(ino, &e, uid, gid), Generation(0)),
                Err(c) => reply.error(errno_of(c)),
            }
        });
    }

    fn getattr(&self, req: &Request, ino: INodeNo, _fh: Option<fuser::FileHandle>, reply: ReplyAttr) {
        let (uid, gid) = (req.uid(), req.gid());
        let ino = ino.0;
        detach!(self, |inner| {
            match inner.entry_of(ino).await {
                Ok(e) => reply.attr(&ATTR_TTL, &attr_of(ino, &e, uid, gid)),
                Err(c) => reply.error(errno_of(c)),
            }
        });
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: fuser::FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let ino = ino.0;
        detach!(self, |inner| {
            let entries = match inner.listing(ino).await {
                Ok(v) => v,
                Err(c) => return reply.error(errno_of(c)),
            };
            // `.` and `..` occupy the first two slots so the rest of the
            // indices line up with the listing. `..` points at this directory
            // for the root, and at *this* directory elsewhere too: the parent
            // is not knowable from an opaque address, and no caller resolves
            // `..` through us — the kernel remembers the path it walked.
            for (i, (child, kind, name)) in std::iter::once((ino, FileType::Directory, ".".into()))
                .chain(std::iter::once((ino, FileType::Directory, "..".into())))
                .chain(entries.iter().map(|e| {
                    (
                        inner.intern(&e.path),
                        if e.is_dir {
                            FileType::Directory
                        } else {
                            FileType::RegularFile
                        },
                        e.name.clone(),
                    )
                }))
                .enumerate()
                .skip(offset as usize)
            {
                // The offset we hand back is where to RESUME, hence i + 1.
                if reply.add(INodeNo(child), i as u64 + 1, kind, &name) {
                    break;
                }
            }
            reply.ok();
        });
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        // The kernel enforces `ro` at the mount before reaching us, so this is
        // belt-and-braces for a caller that got here another way.
        if flags.acc_mode() != OpenAccMode::O_RDONLY {
            return reply.error(Errno::EROFS);
        }
        let ino = ino.0;
        detach!(self, |inner| {
            let Some(path) = inner.path_of(ino) else {
                return reply.error(Errno::ENOENT);
            };
            match inner.gated(inner.remote.open(path)).await {
                Ok((remote, size)) => {
                    let fh = inner.next_fh.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut g) = inner.files.lock() {
                        g.insert(fh, OpenFile { remote, size });
                    }
                    // No flags: keeping the page cache is what lets the kernel
                    // serve a re-read without us, and read ahead of the reader.
                    reply.opened(fuser::FileHandle(fh), FopenFlags::empty());
                }
                Err(c) => reply.error(errno_of(c)),
            }
        });
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: fuser::FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyData,
    ) {
        let fh = fh.0;
        detach!(self, |inner| {
            let Some((remote, file_size)) = inner
                .files
                .lock()
                .ok()
                .and_then(|g| g.get(&fh).map(|f| (f.remote, f.size)))
            else {
                return reply.error(Errno::EBADF);
            };
            // Past the end is an empty read, not an error — and clamping here
            // saves a round trip for the page-sized overshoot the kernel makes
            // at the end of every file.
            if offset >= file_size {
                return reply.data(&[]);
            }
            let want = (file_size - offset).min(size as u64) as u32;
            match inner.read_range(remote, offset, want).await {
                Ok(bytes) => reply.data(&bytes),
                Err(c) => reply.error(errno_of(c)),
            }
        });
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: fuser::FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let fh = fh.0;
        detach!(self, |inner| {
            let handle = inner.files.lock().ok().and_then(|mut g| g.remove(&fh));
            // Answer the kernel first: `close()` cannot fail from here and the
            // caller should not wait on a phone round trip to return from it.
            reply.ok();
            if let Some(f) = handle {
                inner.gated(inner.remote.close(f.remote)).await;
            }
        });
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        // Zeroed on purpose. There is no protocol op for free space, and a made
        // up figure would be a lie a file manager acts on. Zero free on a
        // read-only mount is at least the truth: nothing can be written here.
        // Revisit with design doc §8 step 5, which is when a real number starts
        // to matter.
        reply.statfs(0, 0, 0, 0, 0, 4096, 255, 4096);
    }
}

// ---------------------------------------------------------------------------
// Mount lifecycle
// ---------------------------------------------------------------------------

/// The live session. Dropping it unmounts, which is what cleans up on a normal
/// app exit.
static SESSION: OnceLock<Mutex<Option<fuser::BackgroundSession>>> = OnceLock::new();

fn session_slot() -> &'static Mutex<Option<fuser::BackgroundSession>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

/// Where the phone's files appear.
///
/// Under `XDG_RUNTIME_DIR` because the session lifetime is exactly right: the
/// directory goes away at logout, so a crashed app cannot leave a stale mount
/// point in the user's home. One fixed name rather than one per phone, because
/// the protocol client sends to whichever peer is *active* — a per-phone mount
/// is not expressible until it takes a peer (design doc §9).
pub(crate) fn mount_point() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("vortex").join("phone")
}

/// Mount the phone's storage. Returns the mount point.
///
/// Idempotent: a second call while mounted returns the same path rather than
/// tearing the mount down under whoever is using it.
pub(crate) async fn mount() -> Result<PathBuf, String> {
    if let Ok(g) = session_slot().lock() {
        if g.is_some() {
            return Ok(mount_point());
        }
    }
    let dir = mount_point();
    let rt = tokio::runtime::Handle::current();
    // `Session::new` runs `fusermount3` and waits for the kernel's INIT, so it
    // does not belong on an async thread.
    tokio::task::spawn_blocking(move || mount_blocking(rt, dir))
        .await
        .map_err(|e| format!("mount task failed: {e}"))?
}

fn mount_blocking(rt: tokio::runtime::Handle, dir: PathBuf) -> Result<PathBuf, String> {
    let bg = spawn_session(rt, &dir, LinkRemote)?;
    if let Ok(mut g) = session_slot().lock() {
        *g = Some(bg);
    }
    tracing::info!(path = %dir.display(), "fs-mount: mounted");
    Ok(dir)
}

/// Mount `remote` at `dir` and start serving it.
///
/// Generic over the remote so the integration test at the bottom of this file
/// can mount its fake peer for real — kernel, session thread and all — which is
/// the only way to check that what we tell the kernel is what a program reading
/// the mount actually sees.
fn spawn_session<R: FsRemote>(
    rt: tokio::runtime::Handle,
    dir: &PathBuf,
    remote: R,
) -> Result<fuser::BackgroundSession, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    // A previous run that died without unmounting leaves the path occupied, and
    // the mount would fail with a confusing EBUSY. Only ever our own private
    // path under XDG_RUNTIME_DIR, and a no-op when nothing is mounted there.
    let _ = std::process::Command::new("fusermount3")
        .args(["-quz", &dir.to_string_lossy()])
        .stderr(std::process::Stdio::null())
        .status();

    let fs = PhoneFs {
        inner: Arc::new(Inner::new(remote)),
        rt,
    };
    let mut config = fuser::Config::default();
    config.mount_options = vec![
        // What `mount` and `df` call it.
        MountOption::FSName("vortex".into()),
        MountOption::Subtype("vortex".into()),
        // Read-only until design doc §8 step 5. Enforced by the kernel, so a
        // write is refused without a round trip to the phone.
        MountOption::RO,
        MountOption::NoSuid,
        MountOption::NoDev,
        MountOption::NoExec,
    ];
    // One session thread is enough: it only parses a request and hands it to
    // the runtime, so it is never the thing that is busy.
    fuser::Session::new(fs, dir, &config)
        .map_err(|e| format!("mounting {} failed: {e}", dir.display()))?
        .spawn()
        .map_err(|e| format!("session thread failed: {e}"))
}

/// Unmount, if mounted.
pub(crate) fn unmount() {
    let taken = session_slot().lock().ok().and_then(|mut g| g.take());
    if let Some(bg) = taken {
        // `umount_and_join` waits for the session loop to finish, which needs
        // the kernel to have released the mount — a blocking call, so keep it
        // off the async threads.
        std::thread::spawn(move || match bg.umount_and_join() {
            Ok(()) => tracing::info!("fs-mount: unmounted"),
            Err(e) => tracing::warn!("fs-mount: unmount failed: {e}"),
        });
    }
}

/// Detach the mount on the process's way out.
///
/// A FUSE mount whose server process has died is not gone — it stays in the
/// mount table answering `ENOTCONN`, which makes `df` error and leaves a broken
/// entry in every file manager. So the deliberate-quit path detaches it first.
///
/// `fusermount3 -z` rather than [`unmount`] because this runs microseconds
/// before `exit()`: the lazy form returns immediately and lets the kernel
/// finish when the last user of the mount goes away, where waiting for the
/// session thread to join would simply be killed half-way.
pub(crate) fn unmount_on_exit() {
    if !is_mounted() {
        return;
    }
    let dir = mount_point();
    let _ = std::process::Command::new("fusermount3")
        .args(["-quz", &dir.to_string_lossy()])
        .stderr(std::process::Stdio::null())
        .status();
}

/// Whether the phone's files are currently mounted.
pub(crate) fn is_mounted() -> bool {
    session_slot().lock().map(|g| g.is_some()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A peer with a fixed tree, so the filesystem logic can be tested without
    /// a phone, a kernel or a link. Counts requests: most of what this module
    /// does is avoid making them.
    struct FakePeer {
        /// path → entries, for directories.
        dirs: HashMap<String, Vec<p::FsEntry>>,
        /// path → contents, for files.
        files: HashMap<String, Vec<u8>>,
        lists: std::sync::atomic::AtomicU32,
        stats: std::sync::atomic::AtomicU32,
        reads: std::sync::atomic::AtomicU32,
    }

    fn dir(name: &str, path: &str) -> p::FsEntry {
        p::FsEntry {
            name: name.into(),
            path: path.into(),
            is_dir: true,
            size: 0,
            mtime: 1_700_000_000,
            readonly: true,
        }
    }

    fn file(name: &str, path: &str, size: u64) -> p::FsEntry {
        p::FsEntry {
            name: name.into(),
            path: path.into(),
            is_dir: false,
            size,
            mtime: 1_700_000_000,
            readonly: true,
        }
    }

    impl FakePeer {
        fn new() -> Self {
            // 100 KiB, so a read of it spans three protocol reads.
            let big: Vec<u8> = (0..100 * 1024).map(|i| (i % 251) as u8).collect();
            let mut dirs = HashMap::new();
            dirs.insert(
                String::new(),
                vec![dir("DCIM", "/sdcard/DCIM"), file("a.txt", "/sdcard/a.txt", 5)],
            );
            dirs.insert(
                "/sdcard/DCIM".to_string(),
                vec![file("big.bin", "/sdcard/DCIM/big.bin", big.len() as u64)],
            );
            let mut files = HashMap::new();
            files.insert("/sdcard/a.txt".to_string(), b"hello".to_vec());
            files.insert("/sdcard/DCIM/big.bin".to_string(), big);
            Self {
                dirs,
                files,
                lists: Default::default(),
                stats: Default::default(),
                reads: Default::default(),
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
                let all = me.dirs.get(&path).ok_or(code::NOENT)?;
                // One entry per page, so pagination is exercised rather than
                // assumed.
                let at = cursor as usize;
                match all.get(at) {
                    Some(e) => Ok((
                        vec![e.clone()],
                        (at + 1 < all.len()).then_some(cursor + 1),
                    )),
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
                let bytes = me.files.get(&path).ok_or(code::NOENT)?;
                // The handle IS the path's index; enough to read it back.
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
                let to = (at + len as usize).min(bytes.len());
                Ok((bytes[at..to].to_vec(), to >= bytes.len()))
            }
        }

        async fn close(&self, _handle: u64) {}
    }

    fn fs() -> (Arc<Inner<Arc<FakePeer>>>, Arc<FakePeer>) {
        let peer = Arc::new(FakePeer::new());
        (Arc::new(Inner::new(peer.clone())), peer)
    }

    #[tokio::test]
    async fn listing_follows_pagination_to_the_end() {
        let (fs, peer) = fs();
        let entries = fs.listing(ROOT_INO).await.unwrap();
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["DCIM", "a.txt"]
        );
        // Two pages plus the one that reports the end.
        assert_eq!(peer.lists.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn a_listing_answers_the_lookups_that_follow_it() {
        let (fs, peer) = fs();
        fs.listing(ROOT_INO).await.unwrap();
        let before = peer.lists.load(Ordering::Relaxed);
        let (ino, e) = fs.lookup_child(ROOT_INO, "a.txt").await.unwrap();
        assert_eq!(e.path, "/sdcard/a.txt");
        // The point of the exercise: no further round trips, of any kind.
        assert_eq!(peer.lists.load(Ordering::Relaxed), before);
        assert_eq!(peer.stats.load(Ordering::Relaxed), 0);
        assert_eq!(fs.entry_of(ino).await.unwrap().name, "a.txt");
        assert_eq!(peer.stats.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn an_inode_is_stable_and_never_reused() {
        let (fs, _) = fs();
        let (first, _) = fs.lookup_child(ROOT_INO, "a.txt").await.unwrap();
        let (again, _) = fs.lookup_child(ROOT_INO, "a.txt").await.unwrap();
        assert_eq!(first, again, "the same file must keep its inode number");
        let (other, _) = fs.lookup_child(ROOT_INO, "DCIM").await.unwrap();
        assert_ne!(first, other);
        assert_ne!(first, ROOT_INO);
    }

    #[tokio::test]
    async fn a_missing_name_is_noent_not_a_hang() {
        let (fs, _) = fs();
        assert_eq!(
            fs.lookup_child(ROOT_INO, "nope").await.unwrap_err(),
            code::NOENT
        );
    }

    #[tokio::test]
    async fn the_root_stats_without_asking_the_peer() {
        let (fs, peer) = fs();
        let e = fs.entry_of(ROOT_INO).await.unwrap();
        assert!(e.is_dir);
        assert_eq!(peer.stats.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn a_read_larger_than_the_protocol_limit_is_split_and_reassembled() {
        let (fs, peer) = fs();
        let (_, e) = fs.lookup_child(ROOT_INO, "DCIM").await.unwrap();
        let sub = fs.intern(&e.path);
        let (_, big) = fs.lookup_child(sub, "big.bin").await.unwrap();
        let (handle, size) = fs.remote.open(big.path.clone()).await.unwrap();
        assert_eq!(size, 100 * 1024);

        // One kernel-sized read: 128 KiB clamped to the file, three protocol
        // reads of 48/48/4 KiB.
        let bytes = fs.read_range(handle, 0, size as u32).await.unwrap();
        assert_eq!(bytes.len(), size as usize);
        assert_eq!(peer.reads.load(Ordering::Relaxed), 3);
        let expected: Vec<u8> = (0..100 * 1024).map(|i| (i % 251) as u8).collect();
        assert_eq!(bytes, expected, "reassembled in the wrong order");
    }

    #[tokio::test]
    async fn a_read_at_an_offset_starts_there() {
        let (fs, _) = fs();
        let (handle, size) = fs
            .remote
            .open("/sdcard/DCIM/big.bin".to_string())
            .await
            .unwrap();
        let at = 70 * 1024;
        let bytes = fs.read_range(handle, at, (size - at) as u32).await.unwrap();
        let expected: Vec<u8> = (at..size).map(|i| (i % 251) as u8).collect();
        assert_eq!(bytes, expected);
    }

    #[tokio::test]
    async fn reading_past_the_end_yields_nothing() {
        let (fs, _) = fs();
        let (handle, size) = fs.remote.open("/sdcard/a.txt".to_string()).await.unwrap();
        assert!(fs.read_range(handle, size, 4096).await.unwrap().is_empty());
    }

    #[test]
    fn a_directory_and_a_file_translate_to_the_right_stat() {
        let d = attr_of(7, &dir("DCIM", "/sdcard/DCIM"), 1000, 1000);
        assert_eq!(d.kind, FileType::Directory);
        assert_eq!(d.perm, 0o555);
        assert_eq!(d.ino, INodeNo(7));
        let f = attr_of(8, &file("a.txt", "/sdcard/a.txt", 5), 1000, 1000);
        assert_eq!(f.kind, FileType::RegularFile);
        assert_eq!(f.perm, 0o444, "the mount is read-only");
        assert_eq!(f.size, 5);
        assert_eq!(f.blocks, 1);
        assert_eq!(
            f.mtime,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
        );
    }

    #[test]
    fn an_unknown_mtime_is_the_epoch_not_a_panic() {
        assert_eq!(mtime_of(0), SystemTime::UNIX_EPOCH);
        // A phone with a clock set before 1970 must not take the mount down.
        assert!(mtime_of(-86_400) < SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn every_protocol_code_reaches_the_user_as_itself() {
        assert_eq!(errno_of(code::NOENT), Errno::ENOENT);
        assert_eq!(errno_of(code::ACCES), Errno::EACCES);
        assert_eq!(errno_of(code::ROFS), Errno::EROFS);
        assert_eq!(errno_of(code::ISDIR), Errno::EISDIR);
        assert_eq!(errno_of(crate::fs_link::NO_LINK), Errno::EHOSTDOWN);
        assert_eq!(errno_of(code::IO), Errno::EIO);
        assert_eq!(errno_of(9999), Errno::EIO, "an unknown code is still an error");
    }

    /// The whole thing, for real: a kernel mount over the fake peer, driven by
    /// ordinary `std::fs` calls.
    ///
    /// `#[ignore]` because it needs `/dev/fuse` and `fusermount3`, which a
    /// container or a build box may not have, and a test that cannot run there
    /// should not look like a failure. Run it with:
    ///
    /// ```text
    /// cargo test --lib fs_mount -- --ignored --nocapture
    /// ```
    ///
    /// Multi-threaded on purpose: the syscalls block a thread while the FUSE
    /// operations they trigger are answered on another.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs /dev/fuse and fusermount3"]
    async fn a_real_mount_answers_ordinary_file_calls() {
        let peer = Arc::new(FakePeer::new());
        let dir = std::env::temp_dir().join(format!("vortex-fuse-{}", std::process::id()));
        let session = spawn_session(tokio::runtime::Handle::current(), &dir, peer.clone())
            .expect("mount failed — is /dev/fuse available?");

        let at = dir.clone();
        let seen = tokio::task::spawn_blocking(move || {
            let mut names: Vec<String> = std::fs::read_dir(&at)
                .expect("read_dir")
                .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
                .collect();
            names.sort();
            let small = std::fs::read_to_string(at.join("a.txt")).expect("read a.txt");
            let big = std::fs::read(at.join("DCIM").join("big.bin")).expect("read big.bin");
            let meta = std::fs::metadata(at.join("DCIM")).expect("stat DCIM");
            // Writes must be refused by the kernel, without reaching the peer.
            let write = std::fs::write(at.join("nope.txt"), b"x");
            (names, small, big, meta.is_dir(), write.is_err())
        })
        .await
        .unwrap();

        let (names, small, big, dcim_is_dir, write_refused) = seen;
        assert_eq!(names, ["DCIM", "a.txt"]);
        assert_eq!(small, "hello");
        assert!(dcim_is_dir);
        assert!(write_refused, "the mount must be read-only");
        let expected: Vec<u8> = (0..100 * 1024).map(|i| (i % 251) as u8).collect();
        assert_eq!(big, expected, "100 KiB came back wrong through the kernel");

        tokio::task::spawn_blocking(move || {
            let _ = session.umount_and_join();
            let _ = std::fs::remove_dir(&dir);
        })
        .await
        .unwrap();
    }
}
