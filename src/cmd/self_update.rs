//! `cheni self-update` command.
//!
//! Updates the cheni flake input and rebuilds the system so the new
//! version is available in the PATH.

use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;

use crate::nix::config;

/// Run `cheni self-update`.
///
/// Updates the `cheni` flake input and rebuilds to install the new binary.
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    let config_path = nix_config.flake_dir.to_str()
        .context("Config path is not valid UTF-8")?;

    println!("{}\n", "=== cheni self-update ===".bold());

    // Step 1: Update the cheni flake input
    println!("{} Updating cheni flake input...", "[1/2]".dimmed());

    let update_status = Command::new("nix")
        .args(["flake", "update", "cheni"])
        .current_dir(&nix_config.flake_dir)
        .status()
        .context("Failed to run 'nix flake update cheni'")?;

    if !update_status.success() {
        anyhow::bail!(
            "nix flake update cheni failed.\n\
             Is 'cheni' declared as a flake input in your flake.nix?"
        );
    }

    // Step 2: Rebuild the system to install the new binary
    println!("\n{} Rebuilding system to install new cheni...\n", "[2/2]".dimmed());

    let rebuild_status = Command::new("nh")
        .args(["os", "switch", config_path])
        .status()
        .context("Failed to run 'nh os switch'")?;

    if !rebuild_status.success() {
        anyhow::bail!("System rebuild failed. Run 'cheni build' to see the error.");
    }

    println!("\n{} cheni updated successfully!", "✓".green());
    println!(
        "  New version: {}",
        env!("CARGO_PKG_VERSION").dimmed()
    );

    Ok(())
}
