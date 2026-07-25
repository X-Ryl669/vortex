//! Owner-only filesystem helpers for the on-disk mirror caches.
//!
//! Everything under `~/.cache/vortex/` carries phone-private data (SMS
//! bodies, contacts, call history, app icons). These helpers make sure the
//! directory is 0700 and every file 0600 so other local users can't read
//! them — including repairing permissions left behind by older builds that
//! wrote with the default umask.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Create `dir` (and parents) owner-only. If it already exists, tighten its
/// mode to 0700 — this repairs caches written by older builds.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    // `recursive(true)` applies the mode only to dirs it creates; an existing
    // dir keeps its old (possibly world-readable) mode — fix it explicitly.
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// Write `bytes` to `path` with mode 0600, creating the parent dir 0700.
/// Truncates an existing file and tightens its mode too.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // mode(0o600) only applies on create; an existing file keeps its mode.
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    f.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode_of(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn dir_and_file_are_owner_only() {
        let base = std::env::temp_dir().join(format!("vortex-fsp-{}", std::process::id()));
        let dir = base.join("nested");
        let file = dir.join("data.json");
        write_private(&file, b"x").unwrap();
        assert_eq!(mode_of(&dir), 0o700);
        assert_eq!(mode_of(&file), 0o600);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn repairs_existing_loose_permissions() {
        let base = std::env::temp_dir().join(format!("vortex-fsp-fix-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o755)).unwrap();
        let file = base.join("data.json");
        fs::write(&file, b"old").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();

        write_private(&file, b"new").unwrap();
        assert_eq!(mode_of(&base), 0o700);
        assert_eq!(mode_of(&file), 0o600);
        assert_eq!(fs::read(&file).unwrap(), b"new");
        let _ = fs::remove_dir_all(&base);
    }
}
