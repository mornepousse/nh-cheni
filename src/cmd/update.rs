//! `nixup update` command.
//!
//! Applies all current pins by updating `nixpkgs-latest` and rebuilding
//! the system. This is the command that actually makes changes.

use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use crate::nix::{config, pins};

/// Run `nixup update`.
///
/// 1. Read current pins
/// 2. Update nixpkgs-latest flake input
/// 3. Rebuild the system with nh os switch
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    let current_pins = pins::read(&nix_config.flake_dir)?;

    if current_pins.is_empty() {
        println!("No packages pinned. Nothing to update.");
        println!("Use '{}' to pin packages first.", "nixup pin <pkg>".bold());
        return Ok(());
    }

    println!("{}", "=== nixup update ===\n".bold());

    // Show what will be updated
    println!("Pinned packages:");
    for name in &current_pins {
        println!("  {} {}", "+".green(), name);
    }
    println!();

    // Step 1: Update nixpkgs-latest
    println!(
        "{} Updating nixpkgs-latest...",
        "[1/2]".dimmed()
    );

    let update_status = Command::new("nix")
        .args(["flake", "update", "nixpkgs-latest"])
        .current_dir(&nix_config.flake_dir)
        .status()
        .context("Failed to run 'nix flake update'")?;

    if !update_status.success() {
        anyhow::bail!(
            "nix flake update nixpkgs-latest failed.\n\
             Hint: make sure 'nixpkgs-latest' is defined in your flake.nix.\n\
             Run 'nixup init' to set it up."
        );
    }

    debug!("nixpkgs-latest updated successfully");

    // Step 2: Rebuild
    println!(
        "{} Rebuilding system...\n",
        "[2/2]".dimmed()
    );

    let config_path = nix_config.flake_dir.to_str()
        .context("Config path is not valid UTF-8")?;

    let rebuild_status = Command::new("nh")
        .args(["os", "switch", config_path])
        .status()
        .context("Failed to run 'nh os switch'. Is nh installed?")?;

    if !rebuild_status.success() {
        // Don't remove pins on failure — the user might want to retry
        // or investigate the error.
        anyhow::bail!(
            "System rebuild failed.\n\
             Your pins are still in package-pins.json.\n\
             Fix the issue and run 'nixup update' again, or 'nixup unpin --all' to revert."
        );
    }

    println!(
        "\n{} {} package(s) updated successfully!",
        "✓".green(),
        current_pins.len().to_string().bold()
    );

    Ok(())
}
