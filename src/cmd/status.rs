//! `nixup status` command.
//!
//! Shows the current state: config location, active pins,
//! and system generation info.

use anyhow::Result;
use colored::Colorize;

use crate::nix::{config, pins};

/// Run `nixup status`.
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    let current_pins = pins::read(&nix_config.flake_dir)?;

    // Config info
    println!("{}", "=== nixup status ===\n".bold());
    println!(
        "  {:<16} {}",
        "Config:".dimmed(),
        nix_config.flake_dir.display()
    );
    println!(
        "  {:<16} {}",
        "Hostname:".dimmed(),
        nix_config.hostname
    );

    // Module categories
    let categories = config::list_module_categories(&nix_config.flake_dir);
    if !categories.is_empty() {
        println!(
            "  {:<16} {}",
            "Modules:".dimmed(),
            categories.join(", ")
        );
    }

    // Active pins
    println!();
    if current_pins.is_empty() {
        println!("  {} No active pins.", "●".dimmed());
    } else {
        println!(
            "  {} {} active pin(s):",
            "●".yellow(),
            current_pins.len()
        );
        for name in &current_pins {
            println!("    {} {}", "→".yellow(), name);
        }
    }

    println!();
    Ok(())
}
