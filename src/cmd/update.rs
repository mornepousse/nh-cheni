//! `cheni update` command.
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

/// Run `cheni update`.
///
/// 1. Read current pins
/// 2. Update nixpkgs-latest flake input
/// 3. Verify nixpkgs-latest is ahead of nixpkgs
/// 4. Rebuild the system with nh os switch
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    if !config::is_initialized(&nix_config.flake_dir) {
        super::check::print_first_run_hint();
        return Ok(());
    }

    let current_pins = pins::read(&nix_config.flake_dir)?;
    let flake_lock_dirty = is_flake_lock_dirty(&nix_config.flake_dir);

    if current_pins.is_empty() && !flake_lock_dirty {
        println!("No packages pinned and no pending flake updates.");
        println!("Use '{}' to pin packages first.", "cheni pin <pkg>".bold());
        return Ok(());
    }

    print_update_header(&current_pins, flake_lock_dirty);

    if !current_pins.is_empty() {
        refresh_nixpkgs_latest(&nix_config.flake_dir)?;
        if !verify_nixpkgs_order(&nix_config.flake_dir) {
            return Ok(());
        }
    }

    rebuild_and_announce(&nix_config.flake_dir, current_pins.len())
}

/// Banner + pinned-package list. The dirty-lock hint only fires when
/// there are no pins, since the pin list otherwise already implies the
/// rebuild reason.
fn print_update_header(current_pins: &[String], flake_lock_dirty: bool) {
    println!("{}", "=== cheni update ===\n".bold());
    if !current_pins.is_empty() {
        println!("Pinned packages:");
        for name in current_pins {
            println!("  {} {}", "+".green(), name);
        }
        println!();
    }
    if flake_lock_dirty && current_pins.is_empty() {
        println!("Flake inputs updated — rebuilding.\n");
    }
}

/// Step 1: bump only `nixpkgs-latest` (the per-package overlay source).
fn refresh_nixpkgs_latest(flake_dir: &Path) -> Result<()> {
    println!("{} Updating nixpkgs-latest...", "[1/3]".dimmed());
    let status = Command::new("nix")
        .args(["flake", "update", "nixpkgs-latest"])
        .current_dir(flake_dir)
        .status()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;
    if !status.success() {
        anyhow::bail!(
            "nix flake update nixpkgs-latest failed.\n\
             Hint: make sure 'nixpkgs-latest' is defined in your flake.nix.\n\
             Run 'cheni init' to set it up."
        );
    }
    debug!("nixpkgs-latest updated successfully");
    Ok(())
}

/// Step 2: ensure nixpkgs-latest is actually ahead of nixpkgs.
///
/// Returns true when it's safe to rebuild. Prints its own user-facing
/// guidance and returns false on the two "stop" branches (Same / older).
/// `Unknown` proceeds with a debug warning so a missing/odd flake.lock
/// doesn't strand the user — the rebuild itself will surface real errors.
fn verify_nixpkgs_order(flake_dir: &Path) -> bool {
    println!("{} Checking versions...", "[2/3]".dimmed());
    match check_nixpkgs_order(flake_dir) {
        InputOrder::LatestIsNewer => {
            debug!("nixpkgs-latest is ahead of nixpkgs — safe to apply");
            true
        }
        InputOrder::Same => {
            println!(
                "\n{} nixpkgs and nixpkgs-latest are at the same commit.",
                "!".yellow()
            );
            println!("Pins won't have any effect. Run '{}' to update nixpkgs first.", "upgrade".bold());
            println!("Or '{}' to remove pins.", "cheni unpin --all".bold());
            false
        }
        InputOrder::LatestIsOlder => {
            println!(
                "\n{} nixpkgs-latest is BEHIND nixpkgs — skipping to prevent downgrades.",
                "!".red()
            );
            println!("This can happen after a full '{}'. Pins are no longer needed.", "upgrade".bold());
            println!("Run '{}' to clean up.", "cheni unpin --all".bold());
            false
        }
        InputOrder::Unknown => {
            warn!("Could not compare nixpkgs revisions, proceeding anyway");
            true
        }
    }
}

/// Final step: hand off to `nh os switch`. The custom failure message
/// reminds the user that pins are still on disk, so they can iterate
/// without losing their work.
fn rebuild_and_announce(flake_dir: &Path, pin_count: usize) -> Result<()> {
    println!("{} Rebuilding system...\n", "Rebuilding...".dimmed());
    let config_path = flake_dir.to_str()
        .context("Config path is not valid UTF-8")?;
    let status = Command::new("nh")
        .args(["os", "switch", config_path])
        .status()
        .map_err(|e| crate::nix::tools::tool_error("nh", e))?;
    if !status.success() {
        anyhow::bail!(
            "System rebuild failed.\n\
             Your pins are still in package-pins.json.\n\
             Fix the issue and run 'cheni update' again, or 'cheni unpin --all' to revert."
        );
    }
    println!(
        "\n{} {} package(s) updated successfully!",
        "✓".green(),
        pin_count.to_string().bold()
    );
    Ok(())
}

/// Result of comparing nixpkgs vs nixpkgs-latest revisions.
enum InputOrder {
    /// nixpkgs-latest is at a newer commit (safe to apply pins).
    LatestIsNewer,
    /// Both are at the same commit (pins have no effect).
    Same,
    /// nixpkgs-latest is older (pins would cause downgrades).
    LatestIsOlder,
    /// Could not determine (e.g. can't read flake.lock).
    Unknown,
}

/// Check if nixpkgs-latest is ahead of nixpkgs by comparing their
/// `lastModified` timestamps in flake.lock.
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
///
/// Returns `false` when `git` isn't available, the flake isn't a git
/// repo, or the diff fails — with a debug log in each case so `-v`
/// surfaces the real reason. A silent `false` here would mask a user
/// who just ran `cheni pin --flakes` and expected their changes to be
/// picked up.
fn is_flake_lock_dirty(flake_dir: &Path) -> bool {
    let output = Command::new("git")
        .args(["diff", "--name-only", "flake.lock"])
        .current_dir(flake_dir)
        .output();

    match output {
        Ok(o) if o.status.success() => !o.stdout.is_empty(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            tracing::debug!(
                "git diff returned {:?}, stderr: {}",
                o.status.code(),
                stderr.lines().next().unwrap_or("<empty>")
            );
            false
        }
        Err(e) => {
            tracing::debug!("git not available to check flake.lock dirtiness: {}", e);
            false
        }
    }
}

/// Extract the lastModified timestamp for a flake input from flake.lock.
///
/// Resolves the input via the root node (root.inputs[name]) since the
/// top-level node may be a transitive one, not the root's direct input.
fn get_input_timestamp(lock: &serde_json::Value, input_name: &str) -> Option<u64> {
    // First resolve the actual node name via root.inputs[input_name]
    let root_input = lock.get("nodes")?
        .get("root")?
        .get("inputs")?
        .get(input_name)?;

    let node_name = match root_input.as_str() {
        Some(s) => s,
        None => input_name, // Fallback if the input is inlined
    };

    lock.get("nodes")?
        .get(node_name)?
        .get("locked")?
        .get("lastModified")?
        .as_u64()
}
