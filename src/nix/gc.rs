//! Dry-run preview for `nix-collect-garbage`.
//!
//! The canonical GC commands print one line of interest:
//! `N store paths would be deleted` (dry-run) or `N store paths deleted`
//! (real run). This module wraps the dry-run into a small structured
//! preview so callers (`cheni upgrade --gc`, `cheni history --gc`)
//! can show a user-facing confirmation before actually reclaiming
//! disk space.
//!
//! No sudo is needed for the dry-run itself — the store is
//! world-readable and no profile symlinks are touched. Only the
//! *real* GC step requires root, and that stays in the callers.

use std::process::Command;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;

/// Structured preview of a garbage-collection run.
pub struct GcPreview {
    /// How many store paths the actual run would remove. Parsed from
    /// the dry-run output; `0` when the parser didn't recognise
    /// anything (so callers render an honest zero rather than fail).
    pub paths: usize,
}

static COUNT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d+) store paths? would be deleted").expect("valid regex")
});

/// Run `nix-collect-garbage <extra_args> --dry-run` and return a
/// structured preview. `extra_args` is the caller-specific part
/// (`[]` for the plain GC, `["--delete-older-than", "30d"]` for the
/// upgrade-driven cleanup).
pub fn preview(extra_args: &[&str]) -> Result<GcPreview> {
    let output = Command::new("nix-collect-garbage")
        .args(extra_args)
        .arg("--dry-run")
        .output()
        .map_err(|e| crate::nix::tools::tool_error("nix-collect-garbage", e))?;

    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    combined.push_str(&String::from_utf8_lossy(&output.stderr));

    Ok(GcPreview {
        paths: parse_path_count(&combined),
    })
}

/// Extract the `N store paths would be deleted` count. Returns 0 on
/// parser miss — we'd rather render an honest "no preview available"
/// than abort the whole flow because the output format drifted.
pub fn parse_path_count(output: &str) -> usize {
    COUNT_RE
        .captures(output)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<usize>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "tests/gc.rs"]
mod tests;
