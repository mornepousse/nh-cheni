//! `cheni clean` command.
//!
//! Detects obsolete pins (when nixpkgs has caught up with nixpkgs-latest
//! after a regular `upgrade`) and removes them automatically.
//!
//! With `--orphans`, also removes pins/freezes that no module declares.
//! With `--cruft`, also removes `result*` symlinks in flake_dir and
//! truncates the version cache when over 10 MiB.

use anyhow::Result;
use colored::Colorize;

use crate::nix::{config, pins};

use super::obsolete::count_obsolete_pins;

/// CLI options for `cheni clean`.
#[derive(Debug, Default)]
pub struct CleanOptions {
    /// Remove pins/freezes that no active module declares.
    pub orphans: bool,
    /// Remove `result*` symlinks + truncate oversized version cache.
    pub cruft: bool,
    /// Skip confirmation prompts.
    pub yes: bool,
}

/// Run `cheni clean`.
///
/// Always runs the obsolete phase (default behaviour). The `--orphans`
/// and `--cruft` flags add additional phases, each with its own
/// confirmation prompt.
pub fn run(opts: CleanOptions) -> Result<()> {
    let nix_config = config::detect()?;

    run_obsolete_phase(&nix_config)?;

    // Tasks 3 and 5 will plug the orphan + cruft phases here.
    let _ = opts.orphans;
    let _ = opts.cruft;
    let _ = opts.yes;

    Ok(())
}

/// Drop pins that nixpkgs has caught up on.
fn run_obsolete_phase(nix_config: &config::NixConfig) -> Result<()> {
    let current_pins = pins::read(&nix_config.flake_dir)?;
    if current_pins.is_empty() {
        println!("{} No pins to clean.", "✓".green());
        return Ok(());
    }

    let lock_path = nix_config.flake_dir.join("flake.lock");
    let obsolete_count = count_obsolete_pins(&lock_path, &current_pins);

    if obsolete_count > 0 {
        let count = pins::clear(&nix_config.flake_dir)?;
        println!(
            "{} Removed {} obsolete {}. nixpkgs has caught up with nixpkgs-latest.",
            "✓".green(),
            count.to_string().bold(),
            crate::util::pluralize(count, "pin")
        );
    } else {
        println!(
            "Pins are still active (nixpkgs-latest is ahead). {} {} kept.",
            current_pins.len().to_string().bold(),
            crate::util::pluralize(current_pins.len(), "pin")
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/clean.rs"]
mod tests;
