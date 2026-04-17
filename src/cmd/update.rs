//! `nixup update` command.
//!
//! Applies all current pins by updating `nixpkgs-latest` and rebuilding
//! the system. This is the command that actually makes changes.
//!
//! Before rebuilding, verifies that nixpkgs-latest is actually ahead
//! of nixpkgs to prevent accidental downgrades.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::{debug, warn};

use crate::nix::{config, pins};

/// Run `nixup update`.
///
/// 1. Read current pins
/// 2. Update nixpkgs-latest flake input
/// 3. Verify nixpkgs-latest is ahead of nixpkgs
/// 4. Rebuild the system with nh os switch
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    let current_pins = pins::read(&nix_config.flake_dir)?;

    // Check if flake.lock is dirty (flake inputs were updated via nixup pin)
    let flake_lock_dirty = is_flake_lock_dirty(&nix_config.flake_dir);

    if current_pins.is_empty() && !flake_lock_dirty {
        println!("No packages pinned and no pending flake updates.");
        println!("Use '{}' to pin packages first.", "nixup pin <pkg>".bold());
        return Ok(());
    }

    println!("{}", "=== nixup update ===\n".bold());

    // Show what will be updated
    if !current_pins.is_empty() {
        println!("Pinned packages:");
        for name in &current_pins {
            println!("  {} {}", "+".green(), name);
        }
        println!();
    }

    if flake_lock_dirty && current_pins.is_empty() {
        println!("Flake inputs updated — rebuilding.\n");
    }

    if !current_pins.is_empty() {
        // Step 1: Update nixpkgs-latest (only needed for nixpkgs pins)
        println!(
            "{} Updating nixpkgs-latest...",
            "[1/3]".dimmed()
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

        // Step 2: Verify nixpkgs-latest is ahead of nixpkgs
        println!(
            "{} Checking versions...",
            "[2/3]".dimmed()
        );

    match check_nixpkgs_order(&nix_config.flake_dir) {
        InputOrder::LatestIsNewer => {
            debug!("nixpkgs-latest is ahead of nixpkgs — safe to apply");
        }
        InputOrder::Same => {
            println!(
                "\n{} nixpkgs and nixpkgs-latest are at the same commit.",
                "!".yellow()
            );
            println!("Pins won't have any effect. Run '{}' to update nixpkgs first.", "upgrade".bold());
            println!("Or '{}' to remove pins.", "nixup unpin --all".bold());
            return Ok(());
        }
        InputOrder::LatestIsOlder => {
            println!(
                "\n{} nixpkgs-latest is BEHIND nixpkgs — skipping to prevent downgrades.",
                "!".red()
            );
            println!("This can happen after a full '{}'. Pins are no longer needed.", "upgrade".bold());
            println!("Run '{}' to clean up.", "nixup unpin --all".bold());
            return Ok(());
        }
        InputOrder::Unknown => {
            warn!("Could not compare nixpkgs revisions, proceeding anyway");
        }
    }
    } // end of if !current_pins.is_empty()

    // Rebuild
    println!("{} Rebuilding system...\n", "Rebuilding...".dimmed());

    let config_path = nix_config.flake_dir.to_str()
        .context("Config path is not valid UTF-8")?;

    let rebuild_status = Command::new("nh")
        .args(["os", "switch", config_path])
        .status()
        .context("Failed to run 'nh os switch'. Is nh installed?")?;

    if !rebuild_status.success() {
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

/// Result of comparing nixpkgs vs nixpkgs-latest revisions.
enum InputOrder {
    /// nixpkgs-latest is at a newer commit (safe to apply pins)
    LatestIsNewer,
    /// Both are at the same commit (pins have no effect)
    Same,
    /// nixpkgs-latest is older (pins would cause downgrades)
    LatestIsOlder,
    /// Could not determine (e.g. can't read flake.lock)
    Unknown,
}

/// Check if nixpkgs-latest is ahead of nixpkgs by comparing
/// their lastModified timestamps in flake.lock.
fn check_nixpkgs_order(flake_dir: &Path) -> InputOrder {
    let lock_path = flake_dir.join("flake.lock");
    let content = match std::fs::read_to_string(&lock_path) {
        Ok(c) => c,
        Err(_) => return InputOrder::Unknown,
    };

    let lock: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return InputOrder::Unknown,
    };

    // Extract lastModified timestamps for both inputs
    let nixpkgs_time = get_input_timestamp(&lock, "nixpkgs");
    let latest_time = get_input_timestamp(&lock, "nixpkgs-latest");

    match (nixpkgs_time, latest_time) {
        (Some(base), Some(latest)) => {
            debug!("nixpkgs lastModified: {}, nixpkgs-latest lastModified: {}", base, latest);
            if latest > base {
                InputOrder::LatestIsNewer
            } else if latest == base {
                InputOrder::Same
            } else {
                InputOrder::LatestIsOlder
            }
        }
        _ => {
            debug!("Could not read lastModified from flake.lock");
            InputOrder::Unknown
        }
    }
}

/// Check if flake.lock has uncommitted changes (flake inputs updated).
fn is_flake_lock_dirty(flake_dir: &Path) -> bool {
    let output = Command::new("git")
        .args(["diff", "--name-only", "flake.lock"])
        .current_dir(flake_dir)
        .output();

    match output {
        Ok(o) => !o.stdout.is_empty(),
        Err(_) => false,
    }
}

/// Extract the lastModified timestamp for a flake input from flake.lock.
fn get_input_timestamp(lock: &serde_json::Value, input_name: &str) -> Option<u64> {
    lock.get("nodes")?
        .get(input_name)?
        .get("locked")?
        .get("lastModified")?
        .as_u64()
}
