//! Read versioned files from the user's flake repo as of a given time.
//!
//! Used by features that want to surface "what was true at generation N"
//! (e.g. `cheni history` annotating each generation with the pins/freezes
//! state at that moment). Functions are deliberately non-fatal: missing
//! git, untracked files, or schema drift collapse to "no signal" rather
//! than failing the calling command — this is read-only, optional UI.

use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use tracing::debug;

/// True when `flake.lock` has uncommitted changes inside `flake_dir`.
///
/// Used by every "is the flake state about to surprise me?" surface
/// (`cheni upgrade` preflight warning, `cheni doctor` health check,
/// `cheni status` Suggestions, the interactive banner). Centralised
/// here so all four read the same git output and stay in lockstep.
///
/// Returns `false` when `flake.lock` is clean, when the directory
/// isn't a git work tree, or when `git` itself isn't available —
/// the warning surface is a soft signal, not a gate, so a missing
/// git just means the surface stays silent.
pub fn is_flake_lock_dirty(flake_dir: &Path) -> bool {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(flake_dir)
        .args(["diff", "--name-only", "flake.lock"])
        .output();
    matches!(output, Ok(o) if o.status.success() && !o.stdout.is_empty())
}

/// Whether `dir` is inside a git work tree.
///
/// Callers gate optional features (history annotation, ...) on this
/// check so that users whose NixOS config lives outside git get the
/// regular behaviour without noisy warnings.
pub fn is_repo(dir: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Return `<repo_dir>/<filename>` as it was at-or-before `at`,
/// by walking git history.
///
/// Returns `None` when:
/// - git itself errored or isn't installed
/// - the file wasn't tracked yet at `at` (no commit before that time)
/// - the blob can't be decoded as UTF-8
///
/// `repo_dir` may be the repo root or any subdirectory — we use `git -C`
/// which lets git resolve `<commit>:./<filename>` relative to that dir.
pub fn read_file_at_time(
    repo_dir: &Path,
    filename: &str,
    at: SystemTime,
) -> Option<String> {
    let secs = at.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    let iso = crate::util::format_iso_utc(secs);

    let log_out = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(["log", "-1", "--format=%H"])
        .arg(format!("--before={}", iso))
        .arg("--")
        .arg(filename)
        .output()
        .ok()?;
    if !log_out.status.success() {
        debug!("git log -- {} @ {} failed", filename, iso);
        return None;
    }
    let commit = String::from_utf8(log_out.stdout).ok()?.trim().to_string();
    if commit.is_empty() {
        // No commit touched the file before `at` — file didn't exist yet.
        return None;
    }

    let show_out = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .arg("show")
        .arg(format!("{}:./{}", commit, filename))
        .output()
        .ok()?;
    if !show_out.status.success() {
        debug!("git show {}:./{} failed", commit, filename);
        return None;
    }
    String::from_utf8(show_out.stdout).ok()
}

#[cfg(test)]
#[path = "tests/git.rs"]
mod tests;
