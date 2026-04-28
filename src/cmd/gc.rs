//! `cheni gc` — disk-space orchestrator.
//!
//! Wraps generation pruning + `nix-collect-garbage` with safety
//! guards and a structured preview. See
//! `docs/superpowers/specs/2026-04-28-cheni-gc-design.md`.

#![allow(dead_code, unused_imports)]
// Tasks 3-7 progressively wire each item into call sites and
// remove this module-level allow. Module-scoped is the simplest
// transitional bridge.

use anyhow::Result;

/// Refuse to gc if the user would keep fewer than this — without `--force`.
pub(crate) const MIN_SAFETY_FLOOR: usize = 3;

/// Default number of recent generations to keep.
pub(crate) const DEFAULT_KEEP: usize = 10;

/// CLI options for `cheni gc`.
#[derive(Debug)]
pub struct GcOptions {
    /// Number of recent generations to keep.
    pub keep: usize,
    /// Audit + preview, do not delete anything.
    pub dry_run: bool,
    /// Skip the confirmation prompt.
    pub yes: bool,
    /// Brief output (one-line summary).
    pub brief: bool,
    /// Override the safety floor (allow keep < MIN_SAFETY_FLOOR).
    pub force: bool,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self {
            keep: DEFAULT_KEEP,
            dry_run: false,
            yes: false,
            brief: false,
            force: false,
        }
    }
}

#[cfg(test)]
#[path = "tests/gc.rs"]
mod tests;
