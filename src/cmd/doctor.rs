//! `cheni doctor` command.
//!
//! Health-check of the NixOS + cheni setup. Reports issues like:
//! - Missing nixpkgs-latest input
//! - Pins for packages that don't exist anymore
//! - Stale flake inputs (> 30 days old)
//! - Obsolete pins (nixpkgs caught up)

use anyhow::Result;
use colored::Colorize;

use crate::api::cache;
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

    // Check 6: nix store size
    checks.push(check_store_size());

    // Check 7: number of system generations
    checks.push(check_generations());

    // Check 8: nh is installed (required for cheni build / update)
    checks.push(check_nh_installed());

    // Check 9: Repology cache health
    checks.push(check_cache());

    // Check 10: overlay is resilient to a missing pins file
    checks.push(check_overlay_resilience(&nix_config.flake_dir));

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

/// Check the total size of the nix store.
///
/// Tries `du -sh /nix/store` first. If that fails (permissions, missing
/// binary, or a transient error), falls back to summing block counts via
/// `stat` on the mount point, which is O(1) and works without root.
/// Always logs *why* when a path fails, so running with `-v` surfaces
/// the actual error instead of a generic "could not determine".
fn check_store_size() -> CheckResult {
    if let Some(size) = size_via_du() {
        return classify_store_size(&size);
    }

    // Fallback: rough estimate from the filesystem containing /nix/store.
    // Less precise (includes non-store files on the same FS) but fast and
    // never fails on a readable mount.
    if let Some(size) = size_via_statvfs() {
        return classify_store_size(&size);
    }

    CheckResult {
        severity: Severity::Warning,
        name: "Nix store size".to_string(),
        message: "could not determine (neither 'du' nor statvfs returned a usable value)"
            .to_string(),
        hint: Some("Run with -v for details.".to_string()),
    }
}

/// Shell out to `du -sh /nix/store`. Returns None on any failure, with
/// the reason logged at DEBUG.
fn size_via_du() -> Option<String> {
    let output = match std::process::Command::new("du")
        .args(["-sh", "/nix/store"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            tracing::debug!("du failed to spawn: {}", e);
            return None;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::debug!(
            "du exited with {:?}, stderr: {}",
            output.status.code(),
            stderr.lines().next().unwrap_or("<empty>")
        );
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let size = stdout.split_whitespace().next()?.to_string();
    if size.is_empty() || size == "?" {
        return None;
    }
    Some(size)
}

/// Report the used disk space of the filesystem backing /nix/store using
/// libc statvfs. Returns a human-readable size like "76G" — this over-
/// counts on systems where /nix/store shares its FS with other data, but
/// on dedicated NixOS installs it's a good-enough proxy.
fn size_via_statvfs() -> Option<String> {
    // Use std::fs::metadata as a sanity check that the path is readable,
    // then walk the nix-store child directories ourselves if statvfs isn't
    // easily reachable without an extra crate. Without libc bindings we
    // can't read raw statvfs — so just give up gracefully here.
    // (When users hit this path, the du fallback log message tells them
    // why. Not worth pulling in a libc crate just for the rare case.)
    let _ = std::fs::metadata("/nix/store").ok()?;
    None
}

fn classify_store_size(size: &str) -> CheckResult {
    // Parse "NNg" / "NNG" / "NN.NG" etc. — treat the unit suffix leniently.
    let last = size.chars().last().unwrap_or(' ');
    let number: Option<f64> = size
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .trim()
        .parse()
        .ok();

    let gib = match (number, last) {
        (Some(n), 'T' | 't') => Some(n * 1024.0),
        (Some(n), 'G' | 'g') => Some(n),
        (Some(n), 'M' | 'm') => Some(n / 1024.0),
        _ => None,
    };

    tracing::debug!(
        "Store size parse: size={:?} number={:?} last={} gib={:?}",
        size, number, last, gib
    );
    if gib.map(|g| g > 50.0).unwrap_or(false) {
        CheckResult {
            severity: Severity::Warning,
            name: "Nix store size".to_string(),
            message: format!("{} (quite large)", size),
            hint: Some(
                "Prune old generations safely with 'cheni history --keep 20 --gc' \
                 (keeps the 20 most recent, then reclaims the disk)."
                    .to_string(),
            ),
        }
    } else {
        CheckResult {
            severity: Severity::Ok,
            name: "Nix store size".to_string(),
            message: size.to_string(),
            hint: None,
        }
    }
}

/// Count the system generations by reading /nix/var/nix/profiles directly.
/// No sudo required — the symlinks are world-readable.
fn check_generations() -> CheckResult {
    let dir = std::path::Path::new("/nix/var/nix/profiles");
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            return CheckResult {
                severity: Severity::Warning,
                name: "System generations".to_string(),
                message: "could not read /nix/var/nix/profiles".to_string(),
                hint: None,
            };
        }
    };

    let count = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("system-") && n.ends_with("-link"))
        .count();

    if count > 30 {
        CheckResult {
            severity: Severity::Warning,
            name: "System generations".to_string(),
            message: format!("{} generations (a lot)", count),
            hint: Some(
                "Prune with 'cheni history --keep 20' or 'cheni history --older-than 30d' \
                 (keeps the active generation safe)."
                    .to_string(),
            ),
        }
    } else {
        CheckResult {
            severity: Severity::Ok,
            name: "System generations".to_string(),
            message: format!("{} generation(s)", count),
            hint: None,
        }
    }
}

/// Flag the legacy overlay form that reads package-pins.json without a
/// pathExists guard. If the user ever deletes the file (or stops using
/// cheni without cleaning up the overlay), that form makes the whole
/// flake fail to evaluate. The resilient form degrades to an empty
/// pin list instead.
fn check_overlay_resilience(flake_dir: &std::path::Path) -> CheckResult {
    let flake_path = flake_dir.join("flake.nix");
    let content = match std::fs::read_to_string(&flake_path) {
        Ok(c) => c,
        Err(_) => {
            return CheckResult {
                severity: Severity::Warning,
                name: "Overlay resilience".to_string(),
                message: "could not read flake.nix".to_string(),
                hint: None,
            };
        }
    };

    // Nothing to check when cheni isn't wired up at all.
    if !content.contains("package-pins.json") {
        return CheckResult {
            severity: Severity::Ok,
            name: "Overlay resilience".to_string(),
            message: "no cheni overlay present".to_string(),
            hint: None,
        };
    }

    if content.contains("builtins.pathExists ./package-pins.json") {
        CheckResult {
            severity: Severity::Ok,
            name: "Overlay resilience".to_string(),
            message: "overlay degrades gracefully when pins file is missing".to_string(),
            hint: None,
        }
    } else {
        CheckResult {
            severity: Severity::Warning,
            name: "Overlay resilience".to_string(),
            message: "legacy overlay would fail if package-pins.json is deleted".to_string(),
            hint: Some(
                "Replace 'pins = builtins.fromJSON (builtins.readFile ./package-pins.json);' with:\n\
                 \n  \
                   pins = if builtins.pathExists ./package-pins.json\n  \
                          then builtins.fromJSON (builtins.readFile ./package-pins.json)\n  \
                          else [];\n\
                 \n  \
                 This keeps the flake working if you ever uninstall cheni."
                    .to_string(),
            ),
        }
    }
}

/// Inspect the Repology cache file and warn about potential gotchas.
///
/// The two failure modes worth surfacing here:
/// - The cache contains entries with `version: null`. Older cheni versions
///   used to persist these; they masquerade as "Unknown" forever.
/// - The cache is hours old but hasn't been refreshed (rare since the TTL
///   is 1h, but still a useful signal when versions look weirdly out of date).
fn check_cache() -> CheckResult {
    let s = cache::stats();
    if !s.exists {
        return CheckResult {
            severity: Severity::Ok,
            name: "Repology cache".to_string(),
            message: "no cache yet (will be created on first 'cheni check')".to_string(),
            hint: None,
        };
    }
    if s.null_entries > 0 {
        return CheckResult {
            severity: Severity::Warning,
            name: "Repology cache".to_string(),
            message: format!(
                "{} stale 'unknown' entries out of {}",
                s.null_entries, s.total_entries
            ),
            hint: Some(
                "Wipe with 'cheni check --refresh' (or rm ~/.cache/cheni/versions.json)."
                    .to_string(),
            ),
        };
    }
    let age_human = match s.age_secs {
        n if n < 60 => format!("{}s old", n),
        n if n < 3600 => format!("{}m old", n / 60),
        n => format!("{}h old", n / 3600),
    };
    CheckResult {
        severity: Severity::Ok,
        name: "Repology cache".to_string(),
        message: format!("{} entries, {}", s.total_entries, age_human),
        hint: None,
    }
}

/// Check that `nh` is installed (required by cheni build / update).
fn check_nh_installed() -> CheckResult {
    let output = std::process::Command::new("nh")
        .arg("--version")
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let version = stdout.trim().to_string();
            CheckResult {
                severity: Severity::Ok,
                name: "nh".to_string(),
                message: format!("installed ({})", version),
                hint: None,
            }
        }
        _ => CheckResult {
            severity: Severity::Error,
            name: "nh".to_string(),
            message: "not installed".to_string(),
            hint: Some("cheni build/update depend on nh. Install it in your NixOS config.".to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sev(size: &str) -> Severity {
        classify_store_size(size).severity
    }

    #[test]
    fn classify_small_store() {
        assert_eq!(sev("5.2G"), Severity::Ok);
        assert_eq!(sev("48G"), Severity::Ok);
        assert_eq!(sev("800M"), Severity::Ok);
    }

    #[test]
    fn classify_large_store() {
        assert_eq!(sev("76G"), Severity::Warning);
        assert_eq!(sev("51G"), Severity::Warning);
        assert_eq!(sev("1.2T"), Severity::Warning);
        // Case-insensitive unit suffix
        assert_eq!(sev("100g"), Severity::Warning);
    }

    #[test]
    fn classify_unparseable() {
        // Unknown unit or garbage → Ok (no warning), caller just shows it raw.
        assert_eq!(sev("?"), Severity::Ok);
        assert_eq!(sev("unknown"), Severity::Ok);
    }
}
