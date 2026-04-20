//! `cheni update` command.
//!
//! Applies all current pins by updating `nixpkgs-latest` and rebuilding
//! the system. This is the command that actually makes changes.
//!
//! Before rebuilding, verifies that nixpkgs-latest is actually ahead
//! of nixpkgs to prevent accidental downgrades.

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::{debug, warn};

use crate::nix::{config, pins};

/// Run `cheni update`.
///
/// 1. Update nixpkgs-latest flake input (skipped if no pins)
/// 2. Verify nixpkgs-latest is ahead of nixpkgs
/// 3. Rebuild the system with nh os switch
pub fn run() -> Result<()> {
    let started = Instant::now();
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

    let mut context = UpdateContext::default();

    if !current_pins.is_empty() {
        print_step(1, 3, "Updating nixpkgs-latest");
        let before = read_nixpkgs_latest_timestamp(&nix_config.flake_dir);
        refresh_nixpkgs_latest(&nix_config.flake_dir)?;
        let after = read_nixpkgs_latest_timestamp(&nix_config.flake_dir);
        context.nixpkgs_latest_moved = before != after;
        print_separator();

        print_step(2, 3, "Checking versions");
        if !verify_nixpkgs_order(&nix_config.flake_dir) {
            return Ok(());
        }
        print_separator();
    }

    if let Some(warning) = preview_noop_warning(&context, &current_pins, flake_lock_dirty) {
        println!("  {} {}", "⚠".yellow().bold(), warning.yellow());
        println!();
    }

    print_step(3, 3, "Rebuilding system");
    rebuild(&nix_config.flake_dir)?;
    print_separator();

    print_final_summary(started.elapsed(), current_pins.len(), &context);
    Ok(())
}

/// Signals collected during the run so the final summary (and the
/// pre-rebuild warning) can describe what actually moved.
#[derive(Default)]
struct UpdateContext {
    /// `nixpkgs-latest`'s `lastModified` in flake.lock changed during
    /// step 1. False means `nix flake update` was a no-op.
    nixpkgs_latest_moved: bool,
}

/// Banner + pinned-package list. The dirty-lock hint only fires when
/// there are no pins, since the pin list otherwise already implies the
/// rebuild reason.
fn print_update_header(current_pins: &[String], flake_lock_dirty: bool) {
    println!("{}\n", "=== cheni update ===".bold());
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

/// Render `[N/total] Title` — matches the shape used by `cheni upgrade`.
fn print_step(n: usize, total: usize, title: &str) {
    println!("{} {}", format!("[{}/{}]", n, total).dimmed(), title.bold());
}

/// Horizontal rule between steps — matches `cheni upgrade`.
fn print_separator() {
    println!("{}", "───────────────────────────────────────────".dimmed());
}

/// Step 1: bump only `nixpkgs-latest` (the per-package overlay source).
fn refresh_nixpkgs_latest(flake_dir: &Path) -> Result<()> {
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
    match check_nixpkgs_order(flake_dir) {
        InputOrder::LatestIsNewer => {
            debug!("nixpkgs-latest is ahead of nixpkgs — safe to apply");
            println!("  {} nixpkgs-latest is ahead of nixpkgs.", "✓".green());
            true
        }
        InputOrder::Same => {
            println!(
                "  {} nixpkgs and nixpkgs-latest are at the same commit.",
                "!".yellow()
            );
            println!(
                "  Pins won't have any effect. Run '{}' to update nixpkgs first.",
                "cheni upgrade".bold()
            );
            println!("  Or '{}' to remove pins.", "cheni unpin --all".bold());
            false
        }
        InputOrder::LatestIsOlder => {
            println!(
                "  {} nixpkgs-latest is BEHIND nixpkgs — skipping to prevent downgrades.",
                "!".red()
            );
            println!(
                "  This can happen after a full '{}'. Pins are no longer needed.",
                "cheni upgrade".bold()
            );
            println!("  Run '{}' to clean up.", "cheni unpin --all".bold());
            false
        }
        InputOrder::Unknown => {
            warn!("Could not compare nixpkgs revisions, proceeding anyway");
            println!(
                "  {} Could not compare revisions — proceeding anyway.",
                "·".dimmed()
            );
            true
        }
    }
}

/// Final step: hand off to `nh os switch`. The custom failure message
/// reminds the user that pins are still on disk, so they can iterate
/// without losing their work.
fn rebuild(flake_dir: &Path) -> Result<()> {
    let config_path = flake_dir.to_str().context("Config path is not valid UTF-8")?;
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
    Ok(())
}

/// Pre-rebuild warning: the rebuild is predicted to be a no-op because
/// nothing relevant moved. Returns `None` when there's a genuine reason
/// to rebuild (lock dirty, nixpkgs-latest bumped, or no pins so we
/// can't reason about it).
fn preview_noop_warning(
    context: &UpdateContext,
    current_pins: &[String],
    flake_lock_dirty: bool,
) -> Option<String> {
    // No pins + dirty lock: the rebuild has a real cause (flake.lock
    // changes from outside cheni, e.g. a manual `nix flake update`).
    if current_pins.is_empty() {
        return None;
    }
    if flake_lock_dirty {
        return None;
    }
    if context.nixpkgs_latest_moved {
        return None;
    }
    Some(format!(
        "nixpkgs-latest did not move — {} pin{} already applied at the current closure. \
         Rebuild likely a no-op.",
        current_pins.len(),
        if current_pins.len() == 1 { " is" } else { "s are" },
    ))
}

/// Final summary — truthful even when nothing actually changed. Always
/// shows what the command committed to ("routed through nixpkgs-latest")
/// rather than claiming an update that may have been a no-op.
fn print_final_summary(elapsed: std::time::Duration, pin_count: usize, context: &UpdateContext) {
    let headline = match (pin_count, context.nixpkgs_latest_moved) {
        (0, _) => "flake rebuild complete".to_string(),
        (n, true) => format!(
            "{} pin{} applied from a fresh nixpkgs-latest",
            n,
            if n == 1 { "" } else { "s" },
        ),
        (n, false) => format!(
            "{} pin{} re-applied (nixpkgs-latest unchanged)",
            n,
            if n == 1 { "" } else { "s" },
        ),
    };
    println!(
        "{} {} in {} — {}.",
        "✓".green().bold(),
        "Update complete".bold(),
        format_elapsed(elapsed).dimmed(),
        headline
    );
}

/// Format `Duration` as `MmSs` or `Ss`. Matches the helper in
/// `cheni upgrade` — kept local so each command stays self-contained.
fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

/// Read `nixpkgs-latest`'s `lastModified` timestamp from flake.lock.
/// Returns 0 when the lock can't be read — callers only use this as a
/// "changed?" signal, so a missing-then-present lock will register as
/// "changed", which is the safe default.
fn read_nixpkgs_latest_timestamp(flake_dir: &Path) -> u64 {
    let lock_path = flake_dir.join("flake.lock");
    let content = match std::fs::read_to_string(&lock_path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let lock: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    get_input_timestamp(&lock, "nixpkgs-latest").unwrap_or(0)
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

    let nixpkgs_time = get_input_timestamp(&lock, "nixpkgs");
    let latest_time = get_input_timestamp(&lock, "nixpkgs-latest");

    match (nixpkgs_time, latest_time) {
        (Some(base), Some(latest)) => {
            debug!(
                "nixpkgs lastModified: {}, nixpkgs-latest lastModified: {}",
                base, latest
            );
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
    let root_input = lock
        .get("nodes")?
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

#[cfg(test)]
#[path = "tests/update.rs"]
mod tests;
