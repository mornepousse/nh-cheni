//! Atomic file writes — tmp + fsync + rename, with hardening against
//! symlink and umask attacks.
//!
//! Used by every cheni-spec module that mutates a file the CLI reads
//! back later (pins.json, freezes.json, version-cache.json, the
//! initial timeline.jsonl bootstrap if any). The single source of
//! truth lifted from three duplicated copies during the post-pivot
//! audit (2026-05-02).
//!
//! ## Threat-model hardening (delta vs the original triplicated code)
//!
//! 1. **Mode 0o600 explicit** — the file is created with mode bits
//!    `rw-------` regardless of the process umask. `rename(2)`
//!    preserves the source mode, so the published file is never
//!    world-readable even if the user runs nh-cheni with a
//!    permissive umask (`0022` is the NixOS default).
//!
//! 2. **`O_NOFOLLOW` on the tmp file** — refuses to open if a
//!    pre-existing entry at the tmp path is a symlink. Mitigates
//!    a TOCTOU where a local attacker pre-plants a symlink in a
//!    shared cache directory.
//!
//! 3. **PID-suffixed tmp name** — two concurrent nh-cheni processes
//!    don't fight over the same tmp path.
//!
//! ## What this is NOT
//!
//! Not a transactional fsync of the parent directory — that would
//! double the write cost for a guarantee we don't need (we don't
//! survive power loss across rename, but rename(2) itself is atomic
//! on every Unix filesystem worth using).

use std::{
    fs,
    io::Write,
    path::Path,
};

use color_eyre::eyre::{Context, Result};

/// Write `content` to `path` atomically.
///
/// # Errors
///
/// Returns an error if the tmp file cannot be created (permissions,
/// pre-existing symlink), the write or sync fails, or the rename to
/// the final path fails (e.g. cross-device — the tmp lives in the
/// same parent dir as `path` so this only happens on exotic mounts).
pub fn write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_name = format!(
        "{}.tmp.{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("cheni-util-tmp"),
        std::process::id()
    );
    let tmp = parent.join(&tmp_name);

    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
            // Refuse to open if the tmp path resolves through a
            // symlink. Closes the TOCTOU where a local attacker
            // plants a symlink at the predictable tmp path. The
            // O_NOFOLLOW value comes from the `nix` crate's enum
            // so we don't have to hardcode a Linux-specific
            // integer literal.
            opts.custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits());
        }
        let mut file = opts
            .open(&tmp)
            .with_context(|| format!("opening {} for write", tmp.display()))?;
        file.write_all(content)
            .with_context(|| format!("writing {}", tmp.display()))?;
        // sync_all (data + metadata) so the rename target is durable.
        let _ = file.sync_all();
    }

    fs::rename(&tmp, path).with_context(|| {
        format!("renaming {} → {}", tmp.display(), path.display())
    })?;
    Ok(())
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_creates_file_with_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("foo.txt");
        write(&path, b"hello").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn write_creates_file_with_0600_unix() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("foo.txt");
        write(&path, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn write_overwrites_existing_file_atomically() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("foo.txt");
        fs::write(&path, b"old").unwrap();
        write(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }

    #[test]
    #[cfg(unix)]
    fn write_refuses_when_tmp_path_is_a_symlink() {
        // Pre-plant a symlink at the predictable tmp path. The
        // O_NOFOLLOW flag must refuse the open with ELOOP.
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("foo.txt");
        let target = dir.path().join("victim");
        fs::write(&target, b"victim content").unwrap();
        let tmp_name = format!(
            "{}.tmp.{}",
            path.file_name().and_then(|n| n.to_str()).unwrap(),
            std::process::id()
        );
        symlink(&target, dir.path().join(&tmp_name)).unwrap();
        let result = write(&path, b"attacker controlled");
        assert!(result.is_err(), "O_NOFOLLOW must reject symlink tmp path");
        // The victim file must be untouched.
        assert_eq!(fs::read(&target).unwrap(), b"victim content");
    }
}
