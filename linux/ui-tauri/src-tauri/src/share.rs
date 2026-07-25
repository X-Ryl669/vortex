//! Laptop → phone file sharing (instant-share style, the reverse of the phone→laptop
//! pull). Triggered by the GNOME Files "Share via Vortex" menu, which runs
//! `vortex-ui-tauri --share <paths…>`; the single-instance plugin forwards the
//! argv to the running app, landing here.
//!
//! One "Share" action becomes ONE batch (so the phone shows a single consent
//! prompt and one progress pill): every selected file is read into the batch,
//! and every selected FOLDER is zipped into a single `<name>.zip` archive
//! (Android opens .zip natively). The batch is staged in `core::outgoing_share`;
//! the next LAN heartbeat pushes it after bulk-sync, the phone asks the user to
//! accept, and on accept saves each file to its Downloads. We nudge the
//! heartbeat so it goes out promptly.

use std::io::{Cursor, Read, Write};
use std::path::Path;

use tauri::AppHandle;
use vortex_l3_daemon::core::outgoing_share::{enqueue_batch, OutgoingFile, MAX_PUSH_BYTES};

/// Entry point from the `--share` arg. Builds one batch from all paths (folders
/// zipped) and stages it for the heartbeat push.
pub(crate) fn handle_share(_app: &AppHandle, paths: Vec<String>) {
    let mut batch: Vec<OutgoingFile> = Vec::new();
    for p in &paths {
        let path = Path::new(p);
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(path = %p, "share: cannot stat: {e}");
                continue;
            }
        };
        let prepared = if meta.is_dir() {
            zip_folder(path)
        } else {
            read_file(path)
        };
        match prepared {
            Some(f) if !f.bytes.is_empty() && f.bytes.len() <= MAX_PUSH_BYTES => {
                tracing::info!(name = %f.name, bytes = f.bytes.len(), "share: added to batch");
                batch.push(f);
            }
            Some(f) => {
                tracing::warn!(name = %f.name, len = f.bytes.len(), "share: empty or over cap; skipping");
            }
            None => {}
        }
    }
    if batch.is_empty() {
        tracing::warn!("share: nothing to send");
        return;
    }
    let count = batch.len();
    if enqueue_batch(batch) {
        tracing::info!(count, "share: batch queued for push to phone");
        if let Some(n) = crate::SYNC_NUDGE.get() {
            n.notify_one(); // push now instead of waiting out the heartbeat tick
        }
    } else {
        tracing::warn!(count, "share: batch rejected (empty or over batch cap)");
    }
}

/// Read a single file into an `OutgoingFile`.
fn read_file(path: &Path) -> Option<OutgoingFile> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(path = %path.display(), "share: cannot read: {e}");
            return None;
        }
    };
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "vortex-file".to_string());
    Some(OutgoingFile {
        name,
        mime: "application/octet-stream".to_string(),
        bytes,
        // A user's own file (incl. a real `.zip`) is sent as-is — never unpacked.
        extract: false,
    })
}

/// Zip a folder into a single `<folder>.zip` archive (entries rooted at the
/// folder name) and return it as one `OutgoingFile`. Done in-process with the
/// `zip` crate — no system `zip` binary needed, so it works on any machine.
fn zip_folder(dir: &Path) -> Option<OutgoingFile> {
    let folder_name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "folder".to_string());

    let mut zw = zip::ZipWriter::new(Cursor::new(Vec::<u8>::new()));
    let opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // Walk the tree; archive entries are rooted at the folder's own name
    // (`<folder>/sub/file`), not an absolute path — same layout as `zip -r`.
    let mut count = 0usize;
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        let rel = match path.strip_prefix(dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        // Entries are relative to the shared dir (NO top-folder prefix): the
        // phone re-roots them under Downloads/<folder>/ using the zip's own
        // name, so prefixing here would double-nest (folder/folder/file).
        if rel.as_os_str().is_empty() {
            continue; // the root dir itself
        }
        let arc_name = rel.to_string_lossy().to_string();
        if entry.file_type().is_dir() {
            if zw.add_directory(format!("{arc_name}/"), opts).is_err() {
                tracing::warn!(folder = %folder_name, "share: zip add_directory failed");
                return None;
            }
        } else if entry.file_type().is_file() {
            let mut f = match std::fs::File::open(path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "share: zip skip unreadable: {e}");
                    continue;
                }
            };
            if zw.start_file(&arc_name, opts).is_err() {
                return None;
            }
            let mut buf = Vec::new();
            if f.read_to_end(&mut buf).is_err() || zw.write_all(&buf).is_err() {
                return None;
            }
            count += 1;
        }
    }

    let bytes = zw.finish().ok()?.into_inner();
    if count == 0 {
        tracing::warn!(folder = %folder_name, "share: folder empty; skipping");
        return None;
    }
    tracing::info!(folder = %folder_name, files = count, bytes = bytes.len(), "share: folder zipped (in-process)");
    Some(OutgoingFile {
        name: format!("{folder_name}.zip"),
        mime: "application/zip".to_string(),
        bytes,
        // We made this archive from a folder → the phone unpacks it back to a
        // folder and discards the .zip (seamless folder-share convenience).
        extract: true,
    })
}
