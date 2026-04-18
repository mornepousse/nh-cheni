//! Small utilities shared across commands.
//!
//! Currently just the atomic-write helper. Kept as its own module so we
//! don't accumulate 20 pasted copies of the write-to-tmp-then-rename
//! pattern across `cache`, `pins`, `init`, etc.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

/// Write `content` to `path` atomically on POSIX.
///
/// The file is first written to `<path>.tmp.<pid>`, flushed + synced,
/// then renamed into place. This guarantees:
///
/// - **No partial writes**: if cheni crashes mid-write, the existing
///   file (if any) is left intact.
/// - **No race on concurrent writes**: rename is atomic — readers see
///   either the old contents or the new, never a mix.
/// - **Cross-process safety**: the PID suffix on the tmp file means two
///   `cheni` runs don't fight over the same tmp path.
///
/// Caveat: both the tmp file and the target must live on the same
/// filesystem. All current callers pass paths inside the flake dir or
/// `~/.cache/cheni/`, so that's always true here.
pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    // Same directory as the target so `rename` stays on one FS.
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_name = format!(
        "{}.tmp.{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("cheni-atomic"),
        std::process::id()
    );
    let tmp_path = parent.join(tmp_name);

    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)
            .with_context(|| format!("Failed to open {} for writing", tmp_path.display()))?;
        file.write_all(content.as_bytes())
            .with_context(|| format!("Failed to write {}", tmp_path.display()))?;
        // fsync so the rename doesn't expose a file whose contents are
        // still in the kernel's write-back cache after a power loss.
        file.sync_all().ok();
    }

    std::fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "Failed to rename {} → {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("greeting.txt");
        atomic_write(&path, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn atomic_write_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("counter");
        atomic_write(&path, "1").unwrap();
        atomic_write(&path, "2").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "2");
    }

    #[test]
    fn atomic_write_leaves_no_tmp_files_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("final.txt");
        atomic_write(&path, "clean").unwrap();
        // The only thing in the directory should be the target file.
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], "final.txt");
    }
}
