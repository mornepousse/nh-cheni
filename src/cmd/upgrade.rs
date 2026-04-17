//! `cheni upgrade` command.
//!
//! Full system upgrade: update all flake inputs, rebuild, clean
//! obsolete pins, and garbage-collect old generations.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use crate::nix::{config, pins};

/// Options for `cheni upgrade`.
pub struct UpgradeOptions {
    /// Skip garbage collection at the end.
    pub no_gc: bool,
    /// Skip cleanup of obsolete pins.
    pub no_clean_pins: bool,
}

/// Run `cheni upgrade`.
///
/// Full system upgrade:
/// 1. Update all flake inputs (`nix flake update`)
/// 2. Rebuild the system (`nh os switch`)
/// 3. Clean obsolete pins (`cheni clean` logic)
/// 4. Garbage-collect generations older than 30 days
pub fn run(opts: UpgradeOptions) -> Result<()> {
    let nix_config = config::detect()?;
    let config_path = nix_config.flake_dir.to_str()
        .context("Config path is not valid UTF-8")?;

    println!("{}\n", "=== cheni upgrade ===".bold());

    // Step 1: Update all flake inputs
    println!("{} Updating all flake inputs...", "[1/4]".dimmed());

    let update_status = Command::new("nix")
        .args(["flake", "update"])
        .current_dir(&nix_config.flake_dir)
        .status()
        .context("Failed to run 'nix flake update'")?;

    if !update_status.success() {
        anyhow::bail!("nix flake update failed");
    }

    // Step 2: Rebuild the system
    println!("\n{} Rebuilding system...\n", "[2/4]".dimmed());

    let rebuild_status = Command::new("nh")
        .args(["os", "switch", config_path])
        .status()
        .context("Failed to run 'nh os switch'")?;

    if !rebuild_status.success() {
        anyhow::bail!("System rebuild failed. Fix the issue and run 'cheni build' again.");
    }

    // Step 3: Clean obsolete pins
    if !opts.no_clean_pins {
        println!("\n{} Checking for obsolete pins...", "[3/4]".dimmed());
        clean_obsolete_pins(&nix_config.flake_dir)?;
    } else {
        println!("\n{} {}", "[3/4]".dimmed(), "Skipping pin cleanup (--no-clean-pins)".dimmed());
    }

    // Step 4: Garbage collect
    if !opts.no_gc {
        println!("\n{} Collecting garbage (generations > 30 days)...", "[4/4]".dimmed());
        let gc_status = Command::new("sudo")
            .args(["nix-collect-garbage", "--delete-older-than", "30d"])
            .status()
            .context("Failed to run nix-collect-garbage")?;

        if !gc_status.success() {
            println!("{}", "  (garbage collection skipped or failed)".dimmed());
        }
    } else {
        println!("\n{} {}", "[4/4]".dimmed(), "Skipping garbage collection (--no-gc)".dimmed());
    }

    println!("\n{} Upgrade complete!", "✓".green());
    Ok(())
}

/// Remove pins that are now obsolete (nixpkgs caught up with nixpkgs-latest).
fn clean_obsolete_pins(flake_dir: &Path) -> Result<()> {
    let current_pins = pins::read(flake_dir)?;

    if current_pins.is_empty() {
        println!("  No pins to check.");
        return Ok(());
    }

    let lock_path = flake_dir.join("flake.lock");
    let obsolete = super::obsolete::count_obsolete_pins(&lock_path, &current_pins);

    if obsolete == 0 {
        println!("  All {} pin(s) still needed.", current_pins.len());
        return Ok(());
    }

    // If nixpkgs caught up, all pins are obsolete — clear them all
    let removed = pins::clear(flake_dir)?;
    println!(
        "  {} Removed {} obsolete pin(s).",
        "✓".green(),
        removed
    );
    debug!("Cleaned {} obsolete pins", removed);

    Ok(())
}
