//! Laptop → phone file sharing (instant-share style, the reverse of the phone→laptop
//! pull). Triggered by the GNOME Files "Share via Vortex" menu, which runs
//! `vortex-ui-tauri --share <paths…>`; the single-instance plugin forwards the
//! argv to the running app, landing here.
//!
//! One "Share" action becomes ONE batch (so the phone shows a single consent
//! prompt and one progress pill): every selected file joins the batch as a
//! path, and every selected FOLDER is zipped into a single `<name>.zip`
//! archive on disk (Android opens .zip natively). The batch is staged in
//! `core::outgoing_share`; the next LAN heartbeat offers it, the phone asks the
//! user to accept, and on accept pulls each file in ranges straight into its
//! Downloads. No file is ever read into memory here, so there is no size
//! limit. We nudge the heartbeat so it goes out promptly.
//!
//! Anything that cannot be shared is reported in a desktop notification —
//! never only in the log, which is how an 83 MB PDF used to vanish without a
//! word.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use rand::RngCore;
use tauri::AppHandle;
use vortex_l3_daemon::core::outgoing_share::{self, enqueue_batch, FolderZip, OutgoingFile};

/// Something the user selected that will not reach the phone, and why.
struct Skipped {
    name: String,
    reason: String,
}

/// Entry point from the `--share` arg. Builds one batch from all paths (folders
/// zipped) and stages it for the heartbeat push. The work runs on a blocking
/// thread: zipping a large folder is real disk I/O and must not hold up the
/// single-instance callback that delivered the request.
pub(crate) fn handle_share(_app: &AppHandle, paths: Vec<String>) {
    tauri::async_runtime::spawn_blocking(move || share_paths(paths));
}

fn share_paths(paths: Vec<String>) {
    let mut batch: Vec<OutgoingFile> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    for p in &paths {
        let path = Path::new(p);
        let name = display_name(path);
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(path = %p, "share: cannot stat: {e}");
                skipped.push(Skipped {
                    name,
                    reason: io_reason(&e),
                });
                continue;
            }
        };
        let prepared = if meta.is_dir() {
            zip_folder(path, &mut skipped)
        } else if meta.is_file() {
            prepare_file(path, meta.len())
        } else {
            // A FIFO or a device: reading one blocks or never ends.
            Err("not a regular file".to_string())
        };
        match prepared {
            Ok(f) => {
                tracing::info!(name = %f.name, bytes = f.size, "share: added to batch");
                batch.push(f);
            }
            Err(reason) => {
                tracing::warn!(name = %name, "share: skipping: {reason}");
                skipped.push(Skipped { name, reason });
            }
        }
    }

    if !skipped.is_empty() {
        notify_skipped(&skipped, batch.is_empty());
    }
    if batch.is_empty() {
        tracing::warn!("share: nothing to send");
        return;
    }
    let count = batch.len();
    let label = outgoing_share::batch_label(&batch);
    let total: u64 = batch.iter().map(|f| f.size).sum();
    if enqueue_batch(batch) {
        crate::transfers_out::queued(&label, total);
        tracing::info!(count, "share: batch queued for push to phone");
        if let Some(n) = crate::SYNC_NUDGE.get() {
            n.notify_one(); // push now instead of waiting out the heartbeat tick
        }
    }
}

/// A single file joins the batch as a path. Opened once here so an unreadable
/// file is reported now, on the laptop, rather than as a failed pull later.
fn prepare_file(path: &Path, size: u64) -> Result<OutgoingFile, String> {
    File::open(path).map_err(|e| io_reason(&e))?;
    Ok(OutgoingFile {
        name: display_name(path),
        mime: "application/octet-stream".to_string(),
        path: path.to_path_buf(),
        size,
        // A user's own file (incl. a real `.zip`) is sent as-is — never unpacked.
        folder: None,
    })
}

/// Zip a folder into a single `<folder>.zip` archive in the staging directory
/// and return it as one `OutgoingFile`. Every file is streamed into the archive,
/// so a folder of any size costs disk, not memory. Done in-process with the
/// `zip` crate — no system `zip` binary needed, so it works on any machine.
///
/// Files inside the folder that cannot be read are left out and reported in
/// `skipped`; the rest of the folder still goes.
fn zip_folder(dir: &Path, skipped: &mut Vec<Skipped>) -> Result<OutgoingFile, String> {
    let folder_name = display_name(dir);
    let staging = outgoing_share::temp_dir().ok_or("no cache directory to stage the archive in")?;
    vortex_l3_daemon::core::fs_private::create_private_dir(&staging)
        .map_err(|e| format!("couldn't create {}: {e}", staging.display()))?;
    let mut unique = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut unique);
    let unique: String = unique.iter().map(|b| format!("{b:02x}")).collect();
    let archive = staging.join(outgoing_share::temp_name(&unique));

    match write_zip(dir, &folder_name, &archive, skipped) {
        Ok(zip) => {
            let size = std::fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
            tracing::info!(
                folder = %folder_name,
                files = zip.entries,
                bytes = size,
                "share: folder zipped (in-process, on disk)"
            );
            Ok(OutgoingFile {
                name: format!("{folder_name}.zip"),
                mime: "application/zip".to_string(),
                path: archive,
                size,
                // We made this archive from a folder → the phone unpacks it back
                // to a folder and discards the .zip (seamless folder share).
                folder: Some(zip),
            })
        }
        Err(reason) => {
            let _ = std::fs::remove_file(&archive);
            Err(reason)
        }
    }
}

fn write_zip(
    dir: &Path,
    folder_name: &str,
    archive: &PathBuf,
    skipped: &mut Vec<Skipped>,
) -> Result<FolderZip, String> {
    let out = File::create(archive).map_err(|e| format!("couldn't stage the archive: {e}"))?;
    let mut zw = zip::ZipWriter::new(std::io::BufWriter::new(out));
    let opts: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // Walk the tree; archive entries are relative to the shared dir.
    let mut zip = FolderZip {
        unpacked: 0,
        entries: 0,
    };
    for entry in walkdir::WalkDir::new(dir) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                let name = e
                    .path()
                    .and_then(|p| p.strip_prefix(dir).ok())
                    .map(|r| format!("{folder_name}/{}", r.display()))
                    .unwrap_or_else(|| folder_name.to_string());
                skipped.push(Skipped {
                    name,
                    reason: e
                        .io_error()
                        .map(io_reason)
                        .unwrap_or_else(|| "unreadable".to_string()),
                });
                continue;
            }
        };
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(dir) else {
            continue;
        };
        // Entries are relative to the shared dir (NO top-folder prefix): the
        // phone re-roots them under Downloads/<folder>/ using the zip's own
        // name, so prefixing here would double-nest (folder/folder/file).
        if rel.as_os_str().is_empty() {
            continue; // the root dir itself
        }
        let arc_name = rel.to_string_lossy().to_string();
        if entry.file_type().is_dir() {
            zw.add_directory(format!("{arc_name}/"), opts)
                .map_err(|e| format!("couldn't zip the folder: {e}"))?;
        } else if entry.file_type().is_file() {
            let mut f = match File::open(path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "share: zip skip unreadable: {e}");
                    skipped.push(Skipped {
                        name: format!("{folder_name}/{arc_name}"),
                        reason: io_reason(&e),
                    });
                    continue;
                }
            };
            // Zip64 only where needed, so ordinary archives stay readable by
            // the widest range of unzip tools.
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let o = opts.large_file(len >= u32::MAX as u64);
            zw.start_file(&arc_name, o)
                .map_err(|e| format!("couldn't zip the folder: {e}"))?;
            let n = std::io::copy(&mut f, &mut zw)
                .map_err(|e| format!("couldn't zip '{arc_name}': {}", io_reason(&e)))?;
            zip.unpacked += n;
            zip.entries += 1;
        }
    }
    if zip.entries == 0 {
        return Err("empty folder".to_string());
    }
    let mut w = zw.finish().map_err(|e| format!("couldn't zip the folder: {e}"))?;
    w.flush().map_err(|e| format!("couldn't write the archive: {}", io_reason(&e)))?;
    Ok(zip)
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// An I/O error in words a notification can show.
fn io_reason(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => "not found".to_string(),
        K::PermissionDenied => "permission denied".to_string(),
        _ => e.to_string(),
    }
}

fn notify_skipped(skipped: &[Skipped], nothing_sent: bool) {
    let summary = if nothing_sent {
        "Nothing was sent to the phone".to_string()
    } else if skipped.len() == 1 {
        "1 item wasn't sent to the phone".to_string()
    } else {
        format!("{} items weren't sent to the phone", skipped.len())
    };
    let lines: Vec<String> = skipped
        .iter()
        .map(|s| format!("{} — {}", s.name, s.reason))
        .collect();
    notify_problem(summary, &lines);
}

/// A desktop notification for a share that did not fully happen: a summary,
/// then one line per item. Long lists are cut so the banner stays readable.
pub(crate) fn notify_problem(summary: String, lines: &[String]) {
    const SHOWN: usize = 5;
    let mut body = lines.iter().take(SHOWN).cloned().collect::<Vec<_>>().join("\n");
    if lines.len() > SHOWN {
        body.push_str(&format!("\n…and {} more", lines.len() - SHOWN));
    }
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::notify::show_banner(&summary, &body, "vortex", &[], 0, false).await {
            tracing::warn!("share: couldn't show notification: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vortex-share-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Streams a folder into an archive on disk, entries relative to the
    /// folder, with the totals the phone will hold the unpacking to.
    #[test]
    fn a_folder_zips_to_disk_with_its_totals() {
        let d = scratch("zip");
        let src = d.join("Photos");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"hello").unwrap();
        std::fs::write(src.join("sub/b.bin"), vec![7u8; 100_000]).unwrap();
        let out = d.join("out.zip");
        let mut skipped = Vec::new();

        let z = write_zip(&src, "Photos", &out, &mut skipped).expect("zips");
        assert_eq!((z.entries, z.unpacked), (2, 100_005));
        assert!(skipped.is_empty());

        let mut ar = zip::ZipArchive::new(File::open(&out).unwrap()).unwrap();
        let mut names: Vec<_> = (0..ar.len()).map(|i| ar.by_index(i).unwrap().name().to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["a.txt", "sub/", "sub/b.bin"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_empty_folder_is_refused_with_a_reason() {
        let d = scratch("empty");
        let src = d.join("Nothing");
        std::fs::create_dir_all(src.join("only-dirs")).unwrap();
        let mut skipped = Vec::new();
        let r = write_zip(&src, "Nothing", &d.join("out.zip"), &mut skipped);
        assert_eq!(r.err().as_deref(), Some("empty folder"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_inside_a_folder_is_reported_not_silently_dropped() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("unreadable");
        let src = d.join("Docs");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("ok.txt"), b"ok").unwrap();
        let locked = src.join("locked.txt");
        std::fs::write(&locked, b"secret").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads anything; the check means nothing there.
        if File::open(&locked).is_ok() {
            let _ = std::fs::remove_dir_all(&d);
            return;
        }
        let mut skipped = Vec::new();
        let z = write_zip(&src, "Docs", &d.join("out.zip"), &mut skipped).expect("zips the rest");
        assert_eq!(z.entries, 1);
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "Docs/locked.txt");
        assert_eq!(skipped[0].reason, "permission denied");
        let _ = std::fs::remove_dir_all(&d);
    }
}
