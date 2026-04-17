//! `nixup clean` command.
//!
//! Détecte les pins obsolètes (quand nixpkgs a rattrapé nixpkgs-latest
//! après un `upgrade` classique) et les supprime automatiquement.

use anyhow::Result;
use colored::Colorize;

use crate::nix::{config, pins};

use super::obsolete::count_obsolete_pins;

/// Run `nixup clean`.
///
/// Vérifie si nixpkgs a rattrapé nixpkgs-latest.
/// Si oui, supprime tous les pins (devenus inutiles).
/// Si non, indique que les pins sont encore actifs.
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    let current_pins = pins::read(&nix_config.flake_dir)?;

    if current_pins.is_empty() {
        println!("No pins to clean.");
        return Ok(());
    }

    let lock_path = nix_config.flake_dir.join("flake.lock");
    let obsolete_count = count_obsolete_pins(&lock_path, &current_pins);

    if obsolete_count > 0 {
        // nixpkgs a rattrapé nixpkgs-latest : les pins sont obsolètes
        let count = pins::clear(&nix_config.flake_dir)?;
        println!(
            "{} Removed {} obsolete pin(s). nixpkgs has caught up with nixpkgs-latest.",
            "✓".green(),
            count.to_string().bold()
        );
    } else {
        // nixpkgs-latest est encore devant, les pins sont toujours utiles
        println!(
            "Pins are still active (nixpkgs-latest is ahead). {} pin(s) kept.",
            current_pins.len().to_string().bold()
        );
    }

    Ok(())
}
