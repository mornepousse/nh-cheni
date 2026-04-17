//! `cheni upgrade` command.
//!
//! Full system upgrade: update all flake inputs, rebuild, clean
//! obsolete pins, and optionally garbage-collect old generations.

use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use crate::nix::{config, pins};

/// Options for `cheni upgrade`.
pub struct UpgradeOptions {
    /// Run garbage collection after the rebuild (default: off).
    /// This DELETES old generations — you won't be able to rollback!
    pub gc: bool,
    /// Skip cleanup of obsolete pins.
    pub no_clean_pins: bool,
    /// Skip the preview + confirmation step.
    pub yes: bool,
}

/// Run `cheni upgrade`.
///
/// Full system upgrade:
/// 1. Update all flake inputs (`nix flake update`)
/// 2. Rebuild the system (`nh os switch`)
/// 3. Clean obsolete pins (`cheni clean` logic)
/// 4. (optional, with --gc) Garbage-collect old generations
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

    // Step 1.5: Preview changes (build the new system without activating)
    if !opts.yes {
        println!("\n{} Building new system (preview)...", "[preview]".dimmed());
        println!("{}", "  This evaluates the config without switching.".dimmed());

        let hostname = &nix_config.hostname;
        let flake_ref = format!(
            "{}#nixosConfigurations.{}.config.system.build.toplevel",
            config_path, hostname
        );

        // Build only, no switch — this shows what will change
        let preview_status = Command::new("nh")
            .args(["os", "build", config_path])
            .status()
            .context("Failed to run 'nh os build' for preview")?;

        if !preview_status.success() {
            anyhow::bail!("Preview build failed. Run 'cheni build' to see the error.");
        }

        // Explicitly mention flake_ref is unused (nh handles this internally)
        let _ = flake_ref;

        println!();
        if !confirm("Apply these changes?")? {
            println!("\n{}", "Upgrade cancelled. Flake is already updated.".yellow());
            println!(
                "  Use '{}' to build without switching, or '{}' to rebuild later.",
                "cheni build".bold(),
                "cheni upgrade --yes".bold()
            );
            return Ok(());
        }
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

    // Step 4: Garbage collect (only with --gc flag — keeps rollback safety)
    if opts.gc {
        println!(
            "\n{} {}",
            "[4/4]".dimmed(),
            "Collecting garbage (generations > 30 days)...".yellow()
        );
        println!(
            "  {} This will delete old generations — rollback won't work past this point!",
            "!".yellow()
        );
        let gc_status = Command::new("sudo")
            .args(["nix-collect-garbage", "--delete-older-than", "30d"])
            .status()
            .context("Failed to run nix-collect-garbage")?;

        if !gc_status.success() {
            println!("{}", "  (garbage collection skipped or failed)".dimmed());
        }
    }

    println!("\n{} Upgrade complete!", "✓".green());
    if !opts.gc {
        println!(
            "{}",
            "Old generations kept for rollback. Use --gc to reclaim disk space later.".dimmed()
        );
    }
    Ok(())
}

/// Ask a yes/no question. Default is yes.
fn confirm(question: &str) -> Result<bool> {
    print!("{} {} ", question, "[Y/n]".dimmed());
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();

    Ok(answer.is_empty() || answer == "y" || answer == "yes")
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
