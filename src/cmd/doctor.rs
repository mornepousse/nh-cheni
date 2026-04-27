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
use crate::nix::{config, flake, freezes, pins, store};

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
/// Output is severity-sorted: errors first, then warnings, then a single
/// collapsed line for the OK checks. The user reads what needs attention
/// without scanning through the green-checks list to find it.
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    print_doctor_header(&nix_config);

    let checks = run_all_checks(&nix_config.flake_dir)?;
    let (ok, warn, err) = tally_severities(&checks);

    let mut errors: Vec<&CheckResult> = Vec::new();
    let mut warnings: Vec<&CheckResult> = Vec::new();
    let mut ok_checks: Vec<&CheckResult> = Vec::new();
    for c in &checks {
        match c.severity {
            Severity::Error => errors.push(c),
            Severity::Warning => warnings.push(c),
            Severity::Ok => ok_checks.push(c),
        }
    }
    for c in &errors {
        print_check(c);
    }
    for c in &warnings {
        print_check(c);
    }
    if !ok_checks.is_empty() {
        print_ok_summary(&ok_checks);
    }
    print_summary(ok, warn, err);
    Ok(())
}

/// Render the OK checks as a single collapsed line so the user
/// doesn't have to wade through 10+ green checkmarks to find the
/// items that need attention. Names of the first few checks are
/// listed inline; the rest is a "(+N more)" tail.
fn print_ok_summary(checks: &[&CheckResult]) {
    let display_limit = 4;
    let mut names: Vec<&str> = checks
        .iter()
        .take(display_limit)
        .map(|c| c.name.as_str())
        .collect();
    let body = if checks.len() > display_limit {
        names.push("…");
        format!(
            "{} other check(s) passed ({}, +{} more)",
            checks.len(),
            names[..display_limit].join(", "),
            checks.len() - display_limit,
        )
    } else {
        format!(
            "{} other check(s) passed ({})",
            checks.len(),
            names.join(", "),
        )
    };
    println!("  {}  {}", "✓".green(), body.dimmed());
}

fn print_doctor_header(nix_config: &config::NixConfig) {
    println!("{}\n", "=== cheni doctor ===".bold());
    println!("  Config:   {}", nix_config.flake_dir.display());
    println!("  Hostname: {}\n", nix_config.hostname);
}

/// Run every health check in declared order. Most return one result;
/// a couple (`check_pins_valid`, `check_flake_input_freshness`) fan out
/// to multiple results and use `extend` instead of `push`.
fn run_all_checks(flake_dir: &std::path::Path) -> Result<Vec<CheckResult>> {
    let mut checks = vec![
        check_nixpkgs_latest_input(flake_dir),
        check_nixpkgs_floor_age(flake_dir),
        check_dirty_lock(flake_dir),
        check_pins_file_exists(flake_dir),
    ];
    checks.extend(check_pins_valid(flake_dir)?);
    checks.extend(check_freezes_valid(flake_dir)?);
    checks.extend(check_flake_input_freshness(flake_dir));
    checks.push(check_obsolete_pins(flake_dir));
    checks.push(check_store_size());
    checks.push(check_generations());
    checks.push(check_nh_installed());
    checks.push(check_cache());
    checks.push(check_self_update_available(flake_dir));
    checks.push(check_overlay_resilience(flake_dir));
    Ok(checks)
}

fn tally_severities(checks: &[CheckResult]) -> (usize, usize, usize) {
    let mut ok = 0;
    let mut warn = 0;
    let mut err = 0;
    for c in checks {
        match c.severity {
            Severity::Ok => ok += 1,
            Severity::Warning => warn += 1,
            Severity::Error => err += 1,
        }
    }
    (ok, warn, err)
}

fn print_check(check: &CheckResult) {
    let symbol = match check.severity {
        Severity::Ok => "✓".green(),
        Severity::Warning => "⚠".yellow(),
        Severity::Error => "✗".red(),
    };
    println!("  {}  {} — {}", symbol, check.name.bold(), check.message);
    if let Some(hint) = &check.hint {
        println!("     {} {}", "Hint:".cyan(), hint);
    }
}

fn print_summary(ok_count: usize, warn_count: usize, err_count: usize) {
    println!();
    println!(
        "{} {} passed | {} {} warning(s) | {} {} error(s)",
        "●".green(), ok_count,
        "●".yellow(), warn_count,
        "●".red(), err_count,
    );
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

/// Check that every frozen package is well-formed and still installed.
///
/// Two pathologies worth surfacing here:
/// - **Malformed entries**: a user hand-edit left a non-hex rev or a
///   non-SRI narHash. The overlay would fail at eval. Catching it in
///   `doctor` is a lot friendlier than a cryptic `fetchTree` error on
///   the next rebuild.
/// - **Orphan freezes**: the frozen name is no longer in the store
///   (declaration removed from modules). The freeze has no effect but
///   remains in the JSON file and confuses `cheni status`.
fn check_freezes_valid(flake_dir: &std::path::Path) -> Result<Vec<CheckResult>> {
    let frozen = match freezes::read(flake_dir) {
        Ok(f) => f,
        Err(e) => {
            return Ok(vec![CheckResult {
                severity: Severity::Warning,
                name: "Freezes".to_string(),
                message: format!("could not read package-freezes.json: {}", e),
                hint: Some(
                    "Inspect or reset the file. See the error for the exact path.".to_string(),
                ),
            }]);
        }
    };

    if frozen.is_empty() {
        return Ok(vec![CheckResult {
            severity: Severity::Ok,
            name: "Freezes".to_string(),
            message: "no frozen packages".to_string(),
            hint: None,
        }]);
    }

    let mut results = Vec::new();
    let mut malformed = Vec::new();
    for (name, entry) in &frozen {
        if !is_hex_rev(&entry.rev) || !is_sri_hash(&entry.nar_hash) {
            malformed.push(name.clone());
        }
    }
    if malformed.is_empty() {
        results.push(CheckResult {
            severity: Severity::Ok,
            name: "Freezes validity".to_string(),
            message: format!("{} freeze(s) well-formed", frozen.len()),
            hint: None,
        });
    } else {
        results.push(CheckResult {
            severity: Severity::Warning,
            name: "Malformed freezes".to_string(),
            message: format!(
                "{} freeze(s) with a bad rev or narHash: {}",
                malformed.len(),
                malformed.join(", ")
            ),
            hint: Some(
                "Re-run 'cheni freeze <pkg>' to rewrite the entry with a \
                 fresh rev + narHash fetched from nixpkgs."
                    .to_string(),
            ),
        });
    }

    // Orphan detection: a freeze for a package that's not in the store
    // anymore. Soft warning only — the freeze still works for a name
    // that's about to be re-declared.
    let store_packages = store::read_installed_packages()?;
    let store_names: std::collections::HashSet<String> = store_packages
        .iter()
        .map(|p| p.name.to_lowercase())
        .collect();
    let orphans: Vec<String> = frozen
        .keys()
        .filter(|n| !store_names.contains(&n.to_lowercase()))
        .cloned()
        .collect();
    if !orphans.is_empty() {
        results.push(CheckResult {
            severity: Severity::Warning,
            name: "Orphan freezes".to_string(),
            message: format!(
                "{} freeze(s) for packages not in the store: {}",
                orphans.len(),
                orphans.join(", ")
            ),
            hint: Some("Run 'cheni unfreeze <pkg>' to drop orphan freezes.".to_string()),
        });
    }
    Ok(results)
}

/// Return true when `s` is a plausibly-shaped hex git revision.
fn is_hex_rev(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Return true when `s` looks like an SRI narHash.
fn is_sri_hash(s: &str) -> bool {
    (s.starts_with("sha256-") || s.starts_with("sha512-"))
        && s.len() < 200
        && !s.chars().any(|c| c.is_control() || c == '"' || c == '\\')
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

/// Surface the age of the `nixpkgs` flake input — the foundation
/// for everything else that gets checked.
///
/// Tighter thresholds than the generic `check_flake_input_freshness`
/// (which uses 30 days for any input). nixpkgs-unstable typically
/// bumps daily, so 3+ days is "due", 7+ is "overdue" — and a stale
/// nixpkgs invalidates the assumption of every other check that says
/// "you're up to date".
fn check_nixpkgs_floor_age(flake_dir: &std::path::Path) -> CheckResult {
    let Some(input) = flake::read_input_by_name(flake_dir, "nixpkgs") else {
        return CheckResult {
            severity: Severity::Warning,
            name: "nixpkgs floor".to_string(),
            message: "not found in flake.lock".to_string(),
            hint: Some(
                "Make sure flake.nix declares an `inputs.nixpkgs.url = ...` entry."
                    .to_string(),
            ),
        };
    };
    let days = input.days_old;
    let label = crate::util::format_days_ago(days);
    if days < 3 {
        CheckResult {
            severity: Severity::Ok,
            name: "nixpkgs floor".to_string(),
            message: format!("fresh ({})", label),
            hint: None,
        }
    } else {
        CheckResult {
            severity: Severity::Warning,
            name: "nixpkgs floor".to_string(),
            message: format!("{} — comparison floor is drifting from upstream", label),
            hint: Some("Run 'cheni upgrade' to advance the floor.".to_string()),
        }
    }
}

/// Detect uncommitted changes in `flake.lock` — same trap surfaced
/// by `cheni upgrade`'s preflight, escalated to a doctor-level check
/// so a passive "is my setup healthy?" run catches it before the
/// next rebuild silently applies all the pending bumps.
///
/// Skipped silently when the flake dir isn't inside a git work tree
/// — manual flake setups without git aren't broken, just outside
/// the warning's purview.
fn check_dirty_lock(flake_dir: &std::path::Path) -> CheckResult {
    if !crate::nix::git::is_repo(flake_dir) {
        return CheckResult {
            severity: Severity::Ok,
            name: "flake.lock".to_string(),
            message: "not a git repo (skipped)".to_string(),
            hint: None,
        };
    }
    if crate::nix::git::is_flake_lock_dirty(flake_dir) {
        CheckResult {
            severity: Severity::Warning,
            name: "flake.lock".to_string(),
            message: "uncommitted input changes — next rebuild will apply them".to_string(),
            hint: Some(
                "`git diff flake.lock` to inspect, `git checkout flake.lock` to discard."
                    .to_string(),
            ),
        }
    } else {
        CheckResult {
            severity: Severity::Ok,
            name: "flake.lock".to_string(),
            message: "clean (no uncommitted changes)".to_string(),
            hint: None,
        }
    }
}

/// Compare the user's current cheni pin against the latest release
/// reported by the on-disk cache (filled by `cheni check`'s
/// async path). Sync read — doctor doesn't hit the network on its
/// own, so the answer is only as fresh as the most recent
/// `cheni check`. Skipped silently when the user pins a branch
/// rather than a tag.
fn check_self_update_available(flake_dir: &std::path::Path) -> CheckResult {
    let current_tag = match super::self_update::read_cheni_tag(flake_dir) {
        Ok(t) => t,
        Err(_) => {
            return CheckResult {
                severity: Severity::Ok,
                name: "cheni release".to_string(),
                message: "could not determine current pin (skipped)".to_string(),
                hint: None,
            };
        }
    };
    if !crate::release::is_release_tag(&current_tag) {
        return CheckResult {
            severity: Severity::Ok,
            name: "cheni release".to_string(),
            message: format!("tracking '{}' (not a tag — skipped)", current_tag),
            hint: None,
        };
    }
    let Some(latest) = crate::release::cached_latest_release_tag() else {
        return CheckResult {
            severity: Severity::Ok,
            name: "cheni release".to_string(),
            message: format!("on {} (cache empty — run 'cheni check' to refresh)", current_tag),
            hint: None,
        };
    };
    if latest == current_tag {
        return CheckResult {
            severity: Severity::Ok,
            name: "cheni release".to_string(),
            message: format!("on {} (latest)", current_tag),
            hint: None,
        };
    }
    let cur_v = crate::version::parse::parse_version(
        current_tag.strip_prefix('v').unwrap_or(&current_tag),
    );
    let lat_v =
        crate::version::parse::parse_version(latest.strip_prefix('v').unwrap_or(&latest));
    if lat_v <= cur_v {
        return CheckResult {
            severity: Severity::Ok,
            name: "cheni release".to_string(),
            message: format!("on {} (ahead of cached latest {})", current_tag, latest),
            hint: None,
        };
    }
    CheckResult {
        severity: Severity::Warning,
        name: "cheni release".to_string(),
        message: format!("{} available (you're on {})", latest, current_tag),
        hint: Some("Run 'cheni self-update'.".to_string()),
    }
}

/// Check the total size of the nix store.
///
/// Tries `du -sh /nix/store` and surfaces the exact failure reason
/// (spawn error, exit code, stderr) directly in the warning message
/// so users don't need to remember to re-run with `-v`.
fn check_store_size() -> CheckResult {
    match size_via_du() {
        Ok(size) => classify_store_size(&size),
        Err(reason) => CheckResult {
            severity: Severity::Warning,
            name: "Nix store size".to_string(),
            message: format!("could not determine — {}", reason),
            hint: None,
        },
    }
}

/// Shell out to `du -sh /nix/store`. Returns the parsed size on success,
/// or a short human-readable reason string on any failure — intended to
/// be shown directly to the user rather than hidden behind DEBUG logs.
fn size_via_du() -> Result<String, String> {
    let output = std::process::Command::new("du")
        .args(["-sh", "/nix/store"])
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "'du' binary not in PATH".to_string(),
            _ => format!("failed to run du: {}", e),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first_line = stderr.lines().next().unwrap_or("").trim();
        let detail = if first_line.is_empty() {
            match output.status.code() {
                Some(c) => format!("du exited with code {}", c),
                None => "du terminated without exit code".to_string(),
            }
        } else {
            // Strip a leading "du: " if present — the error is still clear
            // and a bit shorter.
            first_line.strip_prefix("du: ").unwrap_or(first_line).to_string()
        };
        return Err(detail);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let size = stdout.split_whitespace().next()
        .ok_or_else(|| "du returned empty output".to_string())?
        .to_string();
    if size.is_empty() || size == "?" {
        return Err("du returned an unparseable size".to_string());
    }
    Ok(size)
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
#[path = "tests/doctor.rs"]
mod tests;
