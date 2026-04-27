//! Steps 4 and 5 of `cheni upgrade`: prune obsolete pins and
//! optionally garbage-collect old generations.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use crate::nix::pins;

/// Step 4: either clean obsolete pins or announce the skip — `no_clean`
/// decides which branch is taken so the step label stays aligned.
pub(super) fn run_pin_cleanup_step(flake_dir: &Path, no_clean: bool) -> Result<()> {
    if no_clean {
        println!("  {}", "Skipping pin cleanup (--no-clean-pins)".dimmed());
        return Ok(());
    }
    clean_obsolete_pins(flake_dir)
}

/// Remove pins that are now obsolete (nixpkgs caught up with nixpkgs-latest).
fn clean_obsolete_pins(flake_dir: &Path) -> Result<()> {
    let current_pins = pins::read(flake_dir)?;

    if current_pins.is_empty() {
        println!("  {} No pins to check.", "✓".green());
        return Ok(());
    }

    let lock_path = flake_dir.join("flake.lock");
    let obsolete = crate::cmd::obsolete::count_obsolete_pins(&lock_path, &current_pins);

    if obsolete == 0 {
        println!("  All {} still needed.", crate::util::count_phrase(current_pins.len(), "pin"));
        return Ok(());
    }

    // If nixpkgs caught up, all pins are obsolete — clear them all
    let removed = pins::clear(flake_dir)?;
    println!(
        "  {} Removed {} obsolete {}.",
        "✓".green(),
        removed,
        crate::util::pluralize(removed, "pin")
    );
    debug!("Cleaned {} obsolete pins", removed);

    Ok(())
}

/// Step 5: GC generations older than 30 days (only when --gc is set —
/// the rollback guarantee comes from keeping this off by default).
///
/// Previews via `--dry-run` first so the user sees the scope of the
/// deletion (and how many store paths it'll reclaim) before sudo kicks
/// in for the real run. `yes` bypasses the confirmation.
pub(super) fn run_gc_step(yes: bool) -> Result<()> {
    println!(
        "  {} This will delete old generations — rollback won't work past this point!",
        "⚠".yellow()
    );

    let preview = crate::nix::gc::preview(&["--delete-older-than", "30d"])?;
    if preview.paths == 0 {
        println!("  {}", "Nothing older than 30 days to collect.".dimmed());
        return Ok(());
    }
    println!(
        "  {} store {} would be removed.",
        preview.paths.to_string().bold(),
        crate::util::pluralize(preview.paths, "path")
    );

    if !yes && !super::confirm("Proceed with garbage collection?")? {
        println!("{}", "  Cancelled — old generations kept.".yellow());
        return Ok(());
    }

    let status = Command::new("sudo")
        .args(["nix-collect-garbage", "--delete-older-than", "30d"])
        .status()
        .map_err(|e| crate::nix::tools::tool_error("sudo", e))
        .context("running nix-collect-garbage")?;
    if !status.success() {
        println!("{}", "  (garbage collection skipped or failed)".dimmed());
    }
    Ok(())
}
