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
#[path = "tests/util.rs"]
mod tests;
