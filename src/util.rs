//! Small utilities shared across commands.
//!
//! The home of cross-cutting helpers that would otherwise get duplicated
//! across modules: atomic file writes, yes/no confirmation, date
//! formatting, tree-rendering glyphs.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;

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

/// Ask a yes/no question on the terminal and return the answer.
///
/// `default_yes` picks which way a bare Enter goes. The hint rendered
/// next to the prompt matches (`[Y/n]` vs `[y/N]`), so the user can
/// always tell at a glance which side is the safe default. Used by
/// every destructive-or-semi-destructive command (`pin`, `unpin`,
/// `freeze`, `unfreeze`, `upgrade --gc`, ...) so their prompts look
/// identical.
pub fn confirm(question: &str, default_yes: bool) -> Result<bool> {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{} {} ", question, hint.dimmed());
    std::io::stdout().flush().context("flushing stdout")?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("reading confirmation")?;
    let answer = input.trim().to_lowercase();
    if answer.is_empty() {
        return Ok(default_yes);
    }
    Ok(answer == "y" || answer == "yes")
}

/// The tree-branch glyph for position `idx` in a list of `total` items.
/// Returns `"└──"` for the last row and `"├──"` for every other row.
/// Centralises the UTF-8 literals that would otherwise get pasted in
/// every "list with a tree rendering" routine (pin list, freeze list,
/// `cheni why`, ...).
pub fn tree_glyph(idx: usize, total: usize) -> &'static str {
    if idx + 1 == total {
        "└──"
    } else {
        "├──"
    }
}

/// Format a Unix timestamp as `YYYY-MM-DD` (UTC).
///
/// Uses Howard Hinnant's civil-from-days algorithm — enough for a
/// display-only calendar stamp without pulling in the `chrono` crate.
pub fn format_ymd(secs: u64) -> String {
    let (y, m, d) = ymd_from_epoch(secs);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Format a Unix timestamp as `YYYY-MM-DD HH:MM` (UTC). Used by
/// `cheni history` where the minute granularity is worth showing.
pub fn format_ymd_hm(secs: u64) -> String {
    let (y, m, d) = ymd_from_epoch(secs);
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, hours, minutes)
}

/// Convert `secs` since the Unix epoch to `(year, month, day)` in UTC.
/// Pure arithmetic — Howard Hinnant's algorithm, no allocation.
fn ymd_from_epoch(secs: u64) -> (i64, u32, u32) {
    let days_since_epoch = (secs / 86400) as i64;
    let z = days_since_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

#[cfg(test)]
#[path = "tests/util.rs"]
mod tests;
