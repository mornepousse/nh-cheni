//! `cheni preview` command.
//!
//! "What would my next rebuild change?" report. Runs `nix build
//! --dry-run` against the current flake state and renders the same
//! breakdown as `cheni upgrade` step 2 (major/minor/patch/new
//! buckets, plus system artefacts) — without fetching, prompting,
//! or rebuilding.
//!
//! Fills the gap that `cheni check` leaves open: anything coming
//! from `nixpkgs` implicitly (kernel, base system, drivers, …) is
//! not tracked by Repology against a named package in the user's
//! modules, so `check` never surfaces it. `preview` reports on the
//! *closure* level — the actual derivations that would land on the
//! system.
//!
//! Read-only: never modifies `flake.lock`, never invokes the rebuild.

use anyhow::{Context, Result};
use colored::Colorize;

use crate::nix::config;

/// Run `cheni preview`.
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    let config_path = nix_config
        .flake_dir
        .to_str()
        .context("Config path is not valid UTF-8")?;

    println!("{}\n", "=== cheni preview ===".bold());

    // The flake.lock state shapes what the preview means: a clean
    // lock = "if you ran upgrade right now, here's what would
    // happen." A dirty lock = "here's what's queued by a previous
    // run that didn't finish." Same warning as in `cheni upgrade`,
    // since the cause and fix are identical.
    super::upgrade::warn_if_dirty_lock(&nix_config.flake_dir);

    super::upgrade::print_pending_changes(config_path, &nix_config.hostname)?;

    println!();
    println!(
        "  {}",
        "Read-only: nothing was fetched, nothing rebuilt.".dimmed()
    );
    println!(
        "  {} {} to apply  ·  {} to discard the pending bumps",
        "·".dimmed(),
        "cheni upgrade".bold(),
        "git checkout flake.lock".bold(),
    );

    Ok(())
}
