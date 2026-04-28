//! `cheni clean` command.
//!
//! Detects obsolete pins (when nixpkgs has caught up with nixpkgs-latest
//! after a regular `upgrade`) and removes them automatically.
//!
//! With `--orphans`, also removes pins/freezes that no module declares.
//! With `--cruft`, also removes `result*` symlinks in flake_dir and
//! truncates the version cache when over 10 MiB.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm};

use crate::nix::{config, freezes::Freezes, pins};

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

    if opts.orphans {
        run_orphans_phase(&nix_config, opts.yes)?;
    }

    // Task 5 will plug the cruft phase here.
    let _ = opts.cruft;

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

/// Returns the list of pin names that no active module declares.
pub(crate) fn find_orphan_pins(
    pins: &[String],
    declared_packages: &HashSet<String>,
) -> Vec<String> {
    pins.iter()
        .filter(|name| !declared_packages.contains(*name))
        .cloned()
        .collect()
}

/// Returns the list of freeze names that no active module declares.
pub(crate) fn find_orphan_freezes(
    freezes: &Freezes,
    declared_packages: &HashSet<String>,
) -> Vec<String> {
    freezes
        .keys()
        .filter(|name| !declared_packages.contains(*name))
        .cloned()
        .collect()
}

/// Removes the listed orphan pins from `package-pins.json`.
fn apply_remove_orphan_pins(flake_dir: &Path, names: &[String]) -> Result<()> {
    pins::remove(flake_dir, names)?;
    Ok(())
}

/// Removes the listed orphan freezes from `package-freezes.json`.
fn apply_remove_orphan_freezes(flake_dir: &Path, names: &[String]) -> Result<()> {
    crate::nix::freezes::remove(flake_dir, names)?;
    Ok(())
}

fn run_orphans_phase(nix_config: &config::NixConfig, yes: bool) -> Result<()> {
    println!("\n{}", "Orphan pins / freezes:".bold());

    let modules =
        match config::list_active_modules(&nix_config.flake_dir, &nix_config.hostname) {
            Some(m) => m,
            None => {
                println!("{}", "  (no active modules detected — skipping)".dimmed());
                return Ok(());
            }
        };
    let declared: HashSet<String> = config::extract_package_names(&modules)
        .into_iter()
        .collect();

    let current_pins = pins::read(&nix_config.flake_dir)?;
    let current_freezes = crate::nix::freezes::read(&nix_config.flake_dir)?;
    let orphan_pins = find_orphan_pins(&current_pins, &declared);
    let orphan_freezes = find_orphan_freezes(&current_freezes, &declared);

    if orphan_pins.is_empty() && orphan_freezes.is_empty() {
        println!("{} No orphan pins or freezes.", "✓".green());
        return Ok(());
    }

    if !orphan_pins.is_empty() {
        println!(
            "  Found {} orphan pin(s):",
            orphan_pins.len().to_string().bold()
        );
        for name in &orphan_pins {
            println!("    · {}", name);
        }
    }
    if !orphan_freezes.is_empty() {
        println!(
            "  Found {} orphan freeze(s):",
            orphan_freezes.len().to_string().bold()
        );
        for name in &orphan_freezes {
            println!("    · {}", name);
        }
    }

    let proceed = if yes {
        true
    } else {
        Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Remove these orphans?")
            .default(false)
            .interact()
            .map_err(|e| anyhow::anyhow!("reading confirmation: {e}"))?
    };

    if !proceed {
        println!("{}", "  Skipped.".dimmed());
        return Ok(());
    }

    if !orphan_pins.is_empty() {
        apply_remove_orphan_pins(&nix_config.flake_dir, &orphan_pins)?;
        println!(
            "{} Removed {} orphan pin(s).",
            "✓".green(),
            orphan_pins.len()
        );
    }
    if !orphan_freezes.is_empty() {
        apply_remove_orphan_freezes(&nix_config.flake_dir, &orphan_freezes)?;
        println!(
            "{} Removed {} orphan freeze(s).",
            "✓".green(),
            orphan_freezes.len()
        );
    }
    Ok(())
}

/// Threshold above which the version cache is considered oversized
/// and `cheni clean --cruft` proposes truncation.
#[allow(dead_code)]
pub(crate) const VERSION_CACHE_TRUNCATE_THRESHOLD: u64 = 10 * 1024 * 1024;

/// Returns the paths of `result*` symlinks in `flake_dir`.
#[allow(dead_code)]
pub(crate) fn find_result_symlinks(flake_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(flake_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("result") {
                return false;
            }
            e.file_type().map(|t| t.is_symlink()).unwrap_or(false)
        })
        .map(|e| e.path())
        .collect()
}

/// Returns the size in bytes of the version cache, or 0 if missing.
#[allow(dead_code)]
pub(crate) fn version_cache_size_bytes() -> u64 {
    let path = crate::nix::version_cache::cache_path();
    std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
#[path = "tests/clean.rs"]
mod tests;
