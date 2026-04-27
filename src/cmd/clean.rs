//! `cheni clean` command.
//!
//! Detects obsolete pins (when nixpkgs has caught up with nixpkgs-latest
//! after a regular `upgrade`) and removes them automatically.

use anyhow::Result;
use colored::Colorize;

use crate::nix::{config, pins};

use super::obsolete::count_obsolete_pins;

/// Run `cheni clean`.
///
/// Checks whether nixpkgs has caught up with nixpkgs-latest.
/// If so, removes all pins (they are no longer needed).
/// If not, reports that pins are still active.
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    let current_pins = pins::read(&nix_config.flake_dir)?;

    if current_pins.is_empty() {
        println!("{} No pins to clean.", "✓".green());
        return Ok(());
    }

    let lock_path = nix_config.flake_dir.join("flake.lock");
    let obsolete_count = count_obsolete_pins(&lock_path, &current_pins);

    if obsolete_count > 0 {
        // nixpkgs has caught up with nixpkgs-latest: pins are obsolete
        let count = pins::clear(&nix_config.flake_dir)?;
        println!(
            "{} Removed {} obsolete {}. nixpkgs has caught up with nixpkgs-latest.",
            "✓".green(),
            count.to_string().bold(),
            crate::util::pluralize(count, "pin")
        );
    } else {
        // nixpkgs-latest is still ahead, pins are still useful
        println!(
            "Pins are still active (nixpkgs-latest is ahead). {} {} kept.",
            current_pins.len().to_string().bold(),
            crate::util::pluralize(current_pins.len(), "pin")
        );
    }

    Ok(())
}
