//! `nixup status` command.
//!
//! Shows the current state: config location, active pins,
//! and input timestamps. Also warns if pins are obsolete.

use std::path::Path;

use anyhow::Result;
use colored::Colorize;
use tracing::debug;

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

    // Input timestamps
    let lock_path = nix_config.flake_dir.join("flake.lock");
    if let Some((base_date, latest_date)) = read_input_dates(&lock_path) {
        println!(
            "  {:<16} {}",
            "nixpkgs:".dimmed(),
            base_date
        );
        println!(
            "  {:<16} {}",
            "nixpkgs-latest:".dimmed(),
            latest_date
        );
    }

    // Active pins
    println!();
    if current_pins.is_empty() {
        println!("  {} No active pins.", "●".dimmed());
    } else {
        // Check if pins are obsolete (nixpkgs caught up)
        let pins_obsolete = are_pins_obsolete(&lock_path);

        if pins_obsolete {
            println!(
                "  {} {} active pin(s) ({}):",
                "●".red(),
                current_pins.len(),
                "obsolete — nixpkgs caught up".red()
            );
        } else {
            println!(
                "  {} {} active pin(s):",
                "●".yellow(),
                current_pins.len()
            );
        }

        for name in &current_pins {
            println!("    {} {}", "→".yellow(), name);
        }

        if pins_obsolete {
            println!(
                "\n  Run '{}' to clean up obsolete pins.",
                "nixup unpin --all".bold()
            );
        }
    }

    println!();
    Ok(())
}

/// Check if nixpkgs has caught up with nixpkgs-latest (pins are obsolete).
fn are_pins_obsolete(lock_path: &Path) -> bool {
    let content = match std::fs::read_to_string(lock_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let lock: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let base_time = get_input_timestamp(&lock, "nixpkgs");
    let latest_time = get_input_timestamp(&lock, "nixpkgs-latest");

    match (base_time, latest_time) {
        (Some(base), Some(latest)) => {
            debug!("nixpkgs: {}, nixpkgs-latest: {}", base, latest);
            // Pins are obsolete if nixpkgs is at or ahead of nixpkgs-latest
            base >= latest
        }
        _ => false,
    }
}

/// Read human-readable dates from flake.lock for both nixpkgs inputs.
fn read_input_dates(lock_path: &Path) -> Option<(String, String)> {
    let content = std::fs::read_to_string(lock_path).ok()?;
    let lock: serde_json::Value = serde_json::from_str(&content).ok()?;

    let base_time = get_input_timestamp(&lock, "nixpkgs")?;
    let latest_time = get_input_timestamp(&lock, "nixpkgs-latest")?;

    let base_rev = get_input_rev(&lock, "nixpkgs").unwrap_or_default();
    let latest_rev = get_input_rev(&lock, "nixpkgs-latest").unwrap_or_default();

    let base_date = format_timestamp(base_time, &base_rev);
    let latest_date = format_timestamp(latest_time, &latest_rev);

    Some((base_date, latest_date))
}

/// Extract lastModified timestamp for a flake input.
fn get_input_timestamp(lock: &serde_json::Value, name: &str) -> Option<u64> {
    lock.get("nodes")?
        .get(name)?
        .get("locked")?
        .get("lastModified")?
        .as_u64()
}

/// Extract the short rev for a flake input.
fn get_input_rev(lock: &serde_json::Value, name: &str) -> Option<String> {
    let rev = lock.get("nodes")?
        .get(name)?
        .get("locked")?
        .get("rev")?
        .as_str()?;

    Some(rev[..12.min(rev.len())].to_string())
}

/// Format a unix timestamp + short rev into a human-readable string.
fn format_timestamp(ts: u64, rev: &str) -> String {
    // Simple date formatting without pulling in chrono
    let days_ago = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(ts)) / 86400;

    let age = if days_ago == 0 {
        "today".to_string()
    } else if days_ago == 1 {
        "1 day ago".to_string()
    } else {
        format!("{} days ago", days_ago)
    };

    if rev.is_empty() {
        age
    } else {
        format!("{} ({})", age, rev)
    }
}
