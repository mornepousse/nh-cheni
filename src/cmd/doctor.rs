//! `cheni doctor` command.
//!
//! Health-check of the NixOS + cheni setup. Reports issues like:
//! - Missing nixpkgs-latest input
//! - Pins for packages that don't exist anymore
//! - Stale flake inputs (> 30 days old)
//! - Obsolete pins (nixpkgs caught up)

use anyhow::Result;
use colored::Colorize;

use crate::nix::{config, flake, pins, store};

/// Severity of a check result.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Severity {
    /// Something works correctly.
    Ok,
    /// Minor issue, not critical.
    Warning,
    /// Blocking issue that prevents cheni from working.
    Error,
}

/// Result of a single health check.
struct CheckResult {
    severity: Severity,
    name: String,
    message: String,
    hint: Option<String>,
}

/// Run `cheni doctor`.
///
/// Runs a series of health checks and reports issues with severity levels.
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;

    println!("{}\n", "=== cheni doctor ===".bold());
    println!("  Config:   {}", nix_config.flake_dir.display());
    println!("  Hostname: {}\n", nix_config.hostname);

    let mut checks = Vec::new();

    // Check 1: nixpkgs-latest input exists in flake.nix
    checks.push(check_nixpkgs_latest_input(&nix_config.flake_dir));

    // Check 2: package-pins.json exists
    checks.push(check_pins_file_exists(&nix_config.flake_dir));

    // Check 3: pins reference packages that exist in the store
    checks.extend(check_pins_valid(&nix_config.flake_dir)?);

    // Check 4: flake inputs freshness
    checks.extend(check_flake_input_freshness(&nix_config.flake_dir));

    // Check 5: obsolete pins (nixpkgs caught up)
    checks.push(check_obsolete_pins(&nix_config.flake_dir));

    // Print results
    let mut ok_count = 0;
    let mut warn_count = 0;
    let mut err_count = 0;

    for check in &checks {
        let (symbol, color) = match check.severity {
            Severity::Ok => ("✓", "green"),
            Severity::Warning => ("!", "yellow"),
            Severity::Error => ("✗", "red"),
        };

        let colored_symbol = match color {
            "green" => symbol.green(),
            "yellow" => symbol.yellow(),
            "red" => symbol.red(),
            _ => symbol.normal(),
        };

        println!("  {}  {} — {}", colored_symbol, check.name.bold(), check.message);
        if let Some(hint) = &check.hint {
            println!("     {} {}", "Hint:".cyan(), hint);
        }

        match check.severity {
            Severity::Ok => ok_count += 1,
            Severity::Warning => warn_count += 1,
            Severity::Error => err_count += 1,
        }
    }

    println!();
    println!(
        "{} {} passed | {} {} warning(s) | {} {} error(s)",
        "●".green(), ok_count,
        "●".yellow(), warn_count,
        "●".red(), err_count,
    );

    Ok(())
}

/// Check that nixpkgs-latest is declared as a flake input.
fn check_nixpkgs_latest_input(flake_dir: &std::path::Path) -> CheckResult {
    let flake_path = flake_dir.join("flake.nix");
    let content = match std::fs::read_to_string(&flake_path) {
        Ok(c) => c,
        Err(_) => return CheckResult {
            severity: Severity::Error,
            name: "flake.nix".to_string(),
            message: "Cannot read flake.nix".to_string(),
            hint: None,
        },
    };

    if content.contains("nixpkgs-latest") {
        CheckResult {
            severity: Severity::Ok,
            name: "nixpkgs-latest input".to_string(),
            message: "found in flake.nix".to_string(),
            hint: None,
        }
    } else {
        CheckResult {
            severity: Severity::Error,
            name: "nixpkgs-latest input".to_string(),
            message: "not found in flake.nix".to_string(),
            hint: Some("Run 'cheni init' to add it.".to_string()),
        }
    }
}

/// Check that package-pins.json exists.
fn check_pins_file_exists(flake_dir: &std::path::Path) -> CheckResult {
    let pins_path = flake_dir.join("package-pins.json");
    if pins_path.exists() {
        CheckResult {
            severity: Severity::Ok,
            name: "package-pins.json".to_string(),
            message: "exists".to_string(),
            hint: None,
        }
    } else {
        CheckResult {
            severity: Severity::Warning,
            name: "package-pins.json".to_string(),
            message: "not found".to_string(),
            hint: Some("Run 'cheni init' to create it.".to_string()),
        }
    }
}

/// Check that every pinned package name exists in the nix store.
fn check_pins_valid(flake_dir: &std::path::Path) -> Result<Vec<CheckResult>> {
    let pins = pins::read(flake_dir)?;
    if pins.is_empty() {
        return Ok(vec![CheckResult {
            severity: Severity::Ok,
            name: "Pins".to_string(),
            message: "no active pins".to_string(),
            hint: None,
        }]);
    }

    let store_packages = store::read_installed_packages()?;
    let store_names: std::collections::HashSet<String> = store_packages
        .iter()
        .map(|p| p.name.to_lowercase())
        .collect();

    let mut results = Vec::new();
    let mut orphan_pins = Vec::new();

    for pin in &pins {
        if !store_names.contains(&pin.to_lowercase()) {
            orphan_pins.push(pin.clone());
        }
    }

    if orphan_pins.is_empty() {
        results.push(CheckResult {
            severity: Severity::Ok,
            name: "Pins validity".to_string(),
            message: format!("all {} pin(s) point to installed packages", pins.len()),
            hint: None,
        });
    } else {
        results.push(CheckResult {
            severity: Severity::Warning,
            name: "Orphan pins".to_string(),
            message: format!(
                "{} pin(s) for packages not in the store: {}",
                orphan_pins.len(),
                orphan_pins.join(", ")
            ),
            hint: Some("Run 'cheni unpin <pkg>' to remove orphan pins.".to_string()),
        });
    }

    Ok(results)
}

/// Check if any flake input is older than 30 days.
fn check_flake_input_freshness(flake_dir: &std::path::Path) -> Vec<CheckResult> {
    let inputs = match flake::read_flake_inputs(flake_dir) {
        Ok(i) => i,
        Err(_) => return vec![],
    };

    let mut stale = Vec::new();
    for input in &inputs {
        if input.days_old > 30 {
            stale.push(format!("{} ({}d)", input.name, input.days_old));
        }
    }

    if stale.is_empty() {
        vec![CheckResult {
            severity: Severity::Ok,
            name: "Flake input freshness".to_string(),
            message: format!("all {} input(s) updated within 30 days", inputs.len()),
            hint: None,
        }]
    } else {
        vec![CheckResult {
            severity: Severity::Warning,
            name: "Stale flake inputs".to_string(),
            message: format!("inputs older than 30 days: {}", stale.join(", ")),
            hint: Some("Run 'cheni pin --flakes' to see available updates.".to_string()),
        }]
    }
}

/// Check if pins are obsolete (nixpkgs caught up with nixpkgs-latest).
fn check_obsolete_pins(flake_dir: &std::path::Path) -> CheckResult {
    let pins = match pins::read(flake_dir) {
        Ok(p) => p,
        Err(_) => return CheckResult {
            severity: Severity::Warning,
            name: "Obsolete pins check".to_string(),
            message: "could not read pins".to_string(),
            hint: None,
        },
    };

    if pins.is_empty() {
        return CheckResult {
            severity: Severity::Ok,
            name: "Obsolete pins".to_string(),
            message: "none (no active pins)".to_string(),
            hint: None,
        };
    }

    let lock_path = flake_dir.join("flake.lock");
    let obsolete = super::obsolete::count_obsolete_pins(&lock_path, &pins);

    if obsolete == 0 {
        CheckResult {
            severity: Severity::Ok,
            name: "Pins freshness".to_string(),
            message: format!("{} active pin(s), nixpkgs-latest is ahead", pins.len()),
            hint: None,
        }
    } else {
        CheckResult {
            severity: Severity::Warning,
            name: "Obsolete pins".to_string(),
            message: format!("{} pin(s) obsolete — nixpkgs caught up", obsolete),
            hint: Some("Run 'cheni clean' to remove them.".to_string()),
        }
    }
}
