//! `cheni doctor` command.
//!
//! Health-check of the NixOS + cheni setup. Reports issues like:
//! - Missing nixpkgs-latest input
//! - Pins for packages that don't exist anymore
//! - Stale flake inputs (> 30 days old)
//! - Obsolete pins (nixpkgs caught up)

use anyhow::Result;
use colored::Colorize;

use crate::nix::{config, flake, freezes, pins, store};

/// Severity of a check result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Severity {
    /// Something works correctly.
    Ok,
    /// Minor issue, not critical.
    Warning,
    /// Blocking issue that prevents cheni from working.
    Error,
}

/// Result of a single health check.
pub(crate) struct CheckResult {
    pub(crate) severity: Severity,
    pub(crate) name: String,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
}

/// Run `cheni doctor`.
///
/// Runs a series of health checks and reports issues with severity levels.
/// Output is severity-sorted: errors first, then warnings, then a single
/// collapsed line for the OK checks. The user reads what needs attention
/// without scanning through the green-checks list to find it.
///
/// The active-rebuild check is handled separately: when a rebuild is running
/// (Ok with a pid message), it is printed inline before the summary so the
/// user sees it regardless of the `--brief` flag. The idle case collapses
/// into the Ok summary like all other Ok checks.
///
/// When `fix` is true, each warning or error is followed by an interactive
/// prompt to apply a canned fix. Unknown check names show "(no automated fix)".
/// `--brief` and `--fix` are independent — both can be set simultaneously
/// (output is collapsed but prompts still appear).
pub fn run(brief: bool, fix: bool) -> Result<()> {
    let nix_config = config::detect()?;
    if !brief {
        print_doctor_header(&nix_config);
    }

    // Run the active-rebuild check first — it may carry an info-level message
    // that must not be swallowed by the Ok-summary collapse.
    let rebuild_check = check_active_rebuild(&nix_config.flake_dir);
    let rebuild_is_active = rebuild_check.message.contains("pid ");

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

    // Print the active-rebuild result inline when it carries a real signal
    // (rebuild running or stale lock warning). The idle-Ok case falls through
    // to the collapsed summary instead.
    if rebuild_is_active || rebuild_check.severity == Severity::Warning {
        print_check(&rebuild_check, brief);
        if fix && rebuild_check.severity != Severity::Ok {
            apply_fix_interactively(&rebuild_check);
        }
    }

    for c in &errors {
        print_check(c, brief);
        if fix {
            apply_fix_interactively(c);
        }
    }
    for c in &warnings {
        print_check(c, brief);
        if fix {
            apply_fix_interactively(c);
        }
    }
    if !brief && !ok_checks.is_empty() {
        print_ok_summary(&ok_checks);
    }
    print_summary(ok, warn, err);
    Ok(())
}

// ─── Fix dispatcher ────────────────────────────────────────────────────────────

/// Look up the fix function for a known check name.
/// Returns `Some(fn)` when a canned fix is registered, `None` otherwise.
/// Extracted as a pure helper so it can be unit-tested without I/O.
pub(crate) fn fix_fn_for(name: &str) -> Option<fn() -> Result<()>> {
    match name {
        "flake.lock" => Some(fix_flake_lock_dirty),
        "Active rebuild" => Some(fix_dead_upgrade),
        "Nix store size" => Some(fix_store_size),
        "Stale flake inputs" => Some(fix_stale_inputs),
        _ => None,
    }
}

/// Prompt the user to apply the fix for `check`, then run it if confirmed.
/// Unknown check names print "(no automated fix — apply the hint manually)".
///
/// `s for skip-all` is documented in the prompt but is NOT implemented as a
/// session-wide skip state — accepting `s` just skips the current prompt,
/// which is the same as pressing N. Keeping the prompt string consistent
/// with the spec while staying simple.
fn apply_fix_interactively(check: &CheckResult) {
    use colored::Colorize;
    use dialoguer::{theme::ColorfulTheme, Confirm};

    let Some(action) = fix_fn_for(&check.name) else {
        println!("  {} (no automated fix — apply the hint manually)", "·".dimmed());
        return;
    };

    if let Some(hint) = &check.hint {
        println!("  {} {}", "?".cyan(), hint);
    }

    let go = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Apply fix? [y/N/s for skip-all]")
        .default(false)
        .interact();

    match go {
        Ok(true) => {
            if let Err(e) = action() {
                println!("  {} fix failed: {e}", "✗".red());
            }
        }
        Ok(false) => {} // user declined or pressed s
        Err(e) => {
            println!("  {} could not read confirmation: {e}", "✗".red());
        }
    }
    println!();
}

fn fix_flake_lock_dirty() -> Result<()> {
    use colored::Colorize;
    use dialoguer::{theme::ColorfulTheme, Select};

    let theme = ColorfulTheme::default();
    let opts = &[
        "Discard (git checkout flake.lock)",
        "Build now (cheni build)",
        "Skip",
    ];
    let choice = Select::with_theme(&theme)
        .with_prompt("Choose")
        .items(opts)
        .default(2)
        .interact()
        .map_err(|e| anyhow::anyhow!("reading selection: {e}"))?;

    match choice {
        0 => {
            let flake_dir = config::detect()?.flake_dir;
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&flake_dir)
                .args(["checkout", "flake.lock"])
                .status()
                .map_err(|e| crate::nix::tools::tool_error("git", e))?;
            if !status.success() {
                anyhow::bail!("git checkout flake.lock failed");
            }
            println!("  {} flake.lock reset to HEAD.", "✓".green());
        }
        1 => crate::cmd::build::run()?,
        _ => println!("  {}", "Skipped.".dimmed()),
    }
    Ok(())
}

fn fix_dead_upgrade() -> Result<()> {
    use colored::Colorize;
    println!(
        "  {} Running `cheni build` to apply the current flake.lock + config…",
        "→".cyan()
    );
    crate::cmd::build::run()
}

fn fix_store_size() -> Result<()> {
    use colored::Colorize;
    println!("  {} Running `cheni gc` to reclaim disk space…", "→".cyan());
    crate::cmd::gc::run(crate::cmd::gc::GcOptions::default())
}

fn fix_stale_inputs() -> Result<()> {
    use colored::Colorize;
    println!(
        "  {} Run `{}` to see available updates and pick what to update.",
        "→".cyan(),
        "cheni pin --flakes".bold()
    );
    println!(
        "  {} (cheni doctor --fix doesn't auto-run this — choose what you want)",
        "·".dimmed()
    );
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
            "{} other {} passed ({}, +{} more)",
            checks.len(),
            crate::util::pluralize(checks.len(), "check"),
            names[..display_limit].join(", "),
            checks.len() - display_limit,
        )
    } else {
        format!(
            "{} other {} passed ({})",
            checks.len(),
            crate::util::pluralize(checks.len(), "check"),
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
///
/// Note: `check_active_rebuild` is NOT listed here. It is called separately
/// in `run()` because its Ok/info variant needs to bypass the collapsed Ok
/// summary. It IS included in `collect_health` via a direct call so that
/// `cheni audit` also sees it.
fn run_all_checks(flake_dir: &std::path::Path) -> Result<Vec<CheckResult>> {
    let mut checks = vec![
        check_nixpkgs_latest_input(flake_dir),
        check_nixpkgs_floor_age(flake_dir),
        check_dirty_lock(flake_dir),
        check_pins_file_exists(flake_dir),
    ];
    checks.extend(check_pins_valid(flake_dir)?);
    checks.extend(check_freezes_valid(flake_dir)?);
    checks.push(check_pin_freeze_conflict(flake_dir)?);
    checks.extend(check_flake_input_freshness(flake_dir));
    checks.push(check_obsolete_pins(flake_dir));
    checks.push(check_store_size());
    checks.push(check_generations());
    checks.push(check_nh_installed());
    checks.push(check_version_cache());
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

fn print_check(check: &CheckResult, brief: bool) {
    let symbol = match check.severity {
        Severity::Ok => "✓".green(),
        Severity::Warning => "⚠".yellow(),
        Severity::Error => "✗".red(),
    };
    println!("  {}  {} — {}", symbol, check.name.bold(), check.message);
    if !brief {
        if let Some(hint) = &check.hint {
            println!("     {} {}", "Hint:".cyan(), hint);
        }
    }
}

fn print_summary(ok_count: usize, warn_count: usize, err_count: usize) {
    println!();
    println!(
        "{} {} passed | {} {} | {} {}",
        "●".green(), ok_count,
        "●".yellow(), crate::util::count_phrase(warn_count, "warning"),
        "●".red(), crate::util::count_phrase(err_count, "error"),
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
            message: format!("all {} point to installed packages", crate::util::count_phrase(pins.len(), "pin")),
            hint: None,
        });
    } else {
        results.push(CheckResult {
            severity: Severity::Warning,
            name: "Orphan pins".to_string(),
            message: format!(
                "{} for packages not in the store: {}",
                crate::util::count_phrase(orphan_pins.len(), "pin"),
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
            message: format!("{} well-formed", crate::util::count_phrase(frozen.len(), "freeze")),
            hint: None,
        });
    } else {
        results.push(CheckResult {
            severity: Severity::Warning,
            name: "Malformed freezes".to_string(),
            message: format!(
                "{} with a bad rev or narHash: {}",
                crate::util::count_phrase(malformed.len(), "freeze"),
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
                "{} for packages not in the store: {}",
                crate::util::count_phrase(orphans.len(), "freeze"),
                orphans.join(", ")
            ),
            hint: Some("Run 'cheni unfreeze <pkg>' to drop orphan freezes.".to_string()),
        });
    }
    Ok(results)
}

/// Detect packages that appear in both pins and freezes — a corrupt
/// state since the two overlays target the same attribute. Pin/freeze
/// command paths reject the conflict at write-time (see freeze.rs's
/// `reject_if_pinned` and pin.rs's symmetric guard added in v0.5.7),
/// but a hand-edited JSON can still produce a conflict. This check is
/// the safety net that catches it before the next rebuild.
fn check_pin_freeze_conflict(flake_dir: &std::path::Path) -> Result<CheckResult> {
    let pins = pins::read(flake_dir)?;
    let frozen = crate::nix::freezes::read(flake_dir)?;
    let pin_set: std::collections::HashSet<&str> = pins.iter().map(String::as_str).collect();
    let conflicts: Vec<&str> = frozen
        .keys()
        .map(String::as_str)
        .filter(|name| pin_set.contains(name))
        .collect();
    if conflicts.is_empty() {
        Ok(CheckResult {
            severity: Severity::Ok,
            name: "Pin/freeze coherence".to_string(),
            message: "no package is both pinned and frozen".to_string(),
            hint: None,
        })
    } else {
        Ok(CheckResult {
            severity: Severity::Error,
            name: "Pin/freeze conflict".to_string(),
            message: format!(
                "{} pinned AND frozen at the same time: {}",
                conflicts.len(),
                conflicts.join(", ")
            ),
            hint: Some(
                "Edit package-pins.json or package-freezes.json by hand to keep only one. \
                 The two overlays would otherwise race on the same attribute."
                    .to_string(),
            ),
        })
    }
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
            message: format!("all {} updated within 30 days", crate::util::count_phrase(inputs.len(), "input")),
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
            message: format!("{} active, nixpkgs-latest is ahead", crate::util::count_phrase(pins.len(), "pin")),
            hint: None,
        }
    } else {
        CheckResult {
            severity: Severity::Warning,
            name: "Obsolete pins".to_string(),
            message: format!("{} obsolete — nixpkgs caught up", crate::util::count_phrase(obsolete, "pin")),
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
            message: crate::util::count_phrase(count, "generation"),
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

/// Inspect the version cache file and warn about potential issues.
///
/// The two failure modes worth surfacing here:
/// - The cache file exists but cannot be parsed (truncated write, manual edit).
/// - The cache file has grown unexpectedly large (> 10 MiB).
fn check_version_cache() -> CheckResult {
    let path = crate::nix::version_cache::cache_path();
    if !path.exists() {
        return CheckResult {
            severity: Severity::Ok,
            name: "Version cache".to_string(),
            message: "no cache yet (created on first version lookup)".to_string(),
            hint: None,
        };
    }
    let metadata = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) => {
            return CheckResult {
                severity: Severity::Warning,
                name: "Version cache".to_string(),
                message: format!("cannot stat: {e}"),
                hint: None,
            }
        }
    };
    let size_mb = metadata.len() as f64 / 1_048_576.0;
    let parses = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| {
            serde_json::from_str::<crate::nix::version_cache::VersionCache>(&s).ok()
        })
        .is_some();
    let severity = if !parses || size_mb > 10.0 {
        Severity::Warning
    } else {
        Severity::Ok
    };
    let hint = if !parses {
        Some(format!(
            "Cache may be corrupted. Remove it with: rm {}",
            path.display()
        ))
    } else if size_mb > 10.0 {
        Some(format!(
            "Cache is unusually large ({:.2} MiB). Remove it with: rm {}",
            size_mb,
            path.display()
        ))
    } else {
        None
    };
    CheckResult {
        severity,
        name: "Version cache".to_string(),
        message: format!("{:.2} MiB, parses: {}", size_mb, parses),
        hint,
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

/// Collect doctor's findings as structured data, suitable for
/// `cheni audit` composition.
///
/// Mirrors what `run()` does internally, minus printing. Errors / warnings
/// are converted into `audit::HealthIssue` shape. Includes the active-rebuild
/// check (called directly here since it is not in `run_all_checks`).
pub(crate) fn collect_health(
    flake_dir: &std::path::Path,
) -> anyhow::Result<crate::cmd::audit::HealthReport> {
    let mut checks = run_all_checks(flake_dir)?;
    checks.push(check_active_rebuild(flake_dir));
    let mut report = crate::cmd::audit::HealthReport::default();
    for c in &checks {
        let issue = crate::cmd::audit::HealthIssue {
            name: c.name.clone(),
            message: c.message.clone(),
            hint: c.hint.clone(),
        };
        match c.severity {
            Severity::Error => report.errors.push(issue),
            Severity::Warning => report.warnings.push(issue),
            Severity::Ok => report.passed += 1,
        }
    }
    Ok(report)
}

// ─── Active rebuild detection ──────────────────────────────────────────────────

/// Process names that indicate a NixOS rebuild is in progress.
const REBUILD_COMMS: &[&str] = &["nh", "nixos-rebuild", "nix-build"];

/// Return true if `comm` (content of `/proc/<pid>/comm`) matches a known
/// rebuild driver. Case-sensitive — Linux comm values are exact.
pub(crate) fn is_rebuild_comm(comm: &str) -> bool {
    REBUILD_COMMS.contains(&comm)
}

/// Cmdline arguments that identify a `nix-store` invocation as a build step.
const NIX_STORE_BUILD_ARGS: &[&str] = &["--realise", "--realize"];

/// Walk `/proc` and return a description string for the first rebuild process
/// found, or `None` if no rebuild is running.
///
/// Description format: `"pid 12345 — nh os switch (running for 23m)"`.
///
/// Races with process creation/destruction are handled gracefully: any
/// `read_dir` or `read_to_string` failure for a specific PID is silently
/// skipped (`.ok()` + `continue`). The walk only fails if `/proc` itself is
/// unreadable, which on Linux means something is fundamentally wrong.
fn find_active_rebuild() -> Option<String> {
    let proc_dir = std::fs::read_dir("/proc").ok()?;
    // Read uptime once for all duration calculations.
    let uptime_secs = read_uptime_secs();

    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Only numeric entries are PIDs.
        let pid: u64 = match name_str.parse() {
            Ok(n) => n,
            Err(_) => continue,
        };

        let proc_path = entry.path();

        // Read the comm file (process name, max 15 chars on Linux).
        let comm_path = proc_path.join("comm");
        let comm = match std::fs::read_to_string(&comm_path) {
            Ok(c) => c.trim().to_string(),
            Err(_) => continue, // process may have exited
        };

        let matched = if is_rebuild_comm(&comm) {
            true
        } else if comm == "nix-store" {
            // Only flag nix-store when it is doing a realisation, not a query.
            let cmdline_path = proc_path.join("cmdline");
            let cmdline = std::fs::read_to_string(&cmdline_path).unwrap_or_default();
            // cmdline is NUL-separated; treat as space for the purpose of
            // substring matching.
            let cmdline_display = cmdline.replace('\0', " ");
            NIX_STORE_BUILD_ARGS
                .iter()
                .any(|arg| cmdline_display.contains(arg))
        } else {
            false
        };

        if !matched {
            continue;
        }

        // Read the full cmdline for display.
        let cmdline_path = proc_path.join("cmdline");
        let cmdline_raw = std::fs::read_to_string(&cmdline_path).unwrap_or_default();
        let cmdline_display = cmdline_raw
            .replace('\0', " ")
            .trim()
            .to_string();
        // Trim to a reasonable length to avoid huge terminal lines.
        let cmdline_short = if cmdline_display.len() > 80 {
            format!("{}…", &cmdline_display[..80])
        } else {
            cmdline_display
        };

        // Compute running duration from /proc/<pid>/stat field 22 (starttime).
        let duration_str = uptime_secs
            .and_then(|up| read_process_start_ticks(&proc_path).map(|t| (up, t)))
            .map(|(up, start_ticks)| {
                let clock_ticks = clock_ticks_per_sec();
                let start_secs = start_ticks as f64 / clock_ticks as f64;
                let running_secs = (up - start_secs).max(0.0) as u64;
                format_duration(running_secs)
            })
            .unwrap_or_else(|| "?".to_string());

        return Some(format!(
            "pid {} — {} (running for {})",
            pid, cmdline_short, duration_str
        ));
    }
    None
}

/// Read `/proc/uptime` and return the system uptime in seconds.
fn read_uptime_secs() -> Option<f64> {
    let content = std::fs::read_to_string("/proc/uptime").ok()?;
    content
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
}

/// Read field 22 (starttime, 0-indexed as field index 21) from
/// `/proc/<pid>/stat`. Returns clock ticks since boot.
fn read_process_start_ticks(proc_path: &std::path::Path) -> Option<u64> {
    let stat = std::fs::read_to_string(proc_path.join("stat")).ok()?;
    // Field 2 is the comm surrounded by parentheses and may contain spaces.
    // Find the last ')' to skip past it, then count remaining fields.
    let after_comm = stat.rfind(')')?;
    let rest = &stat[after_comm + 1..];
    // Fields after comm: state(1) ppid(2) pgrp(3) session(4) tty_nr(5)
    // tpgid(6) flags(7) minflt(8) cminflt(9) majflt(10) cmajflt(11)
    // utime(12) stime(13) cutime(14) cstime(15) priority(16) nice(17)
    // num_threads(18) itrealvalue(19) starttime(20 — 0-indexed field 19)
    rest.split_whitespace()
        .nth(19) // starttime is the 20th field after ')' (0-indexed: 19)
        .and_then(|s| s.parse::<u64>().ok())
}

/// Return the number of clock ticks per second.
///
/// Reads `/proc/self/status` first (not useful for this). Instead, we try
/// to parse the kernel config via `getconf CLK_TCK`. If that fails we fall
/// back to 100, which is the default on all mainstream Linux kernels and
/// correct for the vast majority of NixOS systems. A wrong value here only
/// affects the "running for Xm Ys" display string, not correctness of the
/// check itself.
fn clock_ticks_per_sec() -> u64 {
    // Try `getconf CLK_TCK` — available on all POSIX systems without libc FFI.
    let output = std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .ok();
    if let Some(out) = output {
        if out.status.success() {
            if let Ok(s) = std::str::from_utf8(&out.stdout) {
                if let Ok(n) = s.trim().parse::<u64>() {
                    if n > 0 {
                        return n;
                    }
                }
            }
        }
    }
    100 // near-universal Linux default
}

/// Format a duration in seconds as "Xm Ys" or "Xs" for display.
fn format_duration(secs: u64) -> String {
    if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

/// Pure classification logic — given what the I/O layer found, return
/// the appropriate `CheckResult`. Separated for unit-testability.
///
/// `lock_dirty` = `flake.lock` has uncommitted changes per `git status`.
/// Combined with "no active rebuild", this means a previous `cheni upgrade`
/// updated the lock but didn't reach the rebuild step (likely killed mid-flight).
pub(crate) fn classify_rebuild_state(
    active: Option<String>,
    lock_dirty: bool,
) -> CheckResult {
    match (active, lock_dirty) {
        (Some(detail), _) => CheckResult {
            severity: Severity::Ok,
            name: "Active rebuild".to_string(),
            message: detail,
            hint: None,
        },
        (None, true) => CheckResult {
            severity: Severity::Warning,
            name: "Active rebuild".to_string(),
            message: "flake.lock has uncommitted changes but no rebuild is running".to_string(),
            hint: Some(
                "Previous `cheni upgrade` likely died. \
                 Run `cheni build` to apply the current flake.lock + config, \
                 or `git checkout flake.lock` to discard."
                    .to_string(),
            ),
        },
        (None, false) => CheckResult {
            severity: Severity::Ok,
            name: "Active rebuild".to_string(),
            message: "no rebuild in progress".to_string(),
            hint: None,
        },
    }
}

/// I/O wrapper: walk `/proc` and check `flake.lock` git state, then
/// delegate to `classify_rebuild_state` for the pure logic.
///
/// We use `git diff` rather than mtime because mtime gives false
/// negatives for upgrades that started > 1h ago (the user may run
/// `cheni doctor` 2h after killing a stuck upgrade — mtime says "old"
/// but the lock IS still uncommitted, which is the real signal).
fn check_active_rebuild(flake_dir: &std::path::Path) -> CheckResult {
    let active = find_active_rebuild();
    let lock_dirty = crate::nix::git::is_flake_lock_dirty(flake_dir);
    classify_rebuild_state(active, lock_dirty)
}

#[cfg(test)]
#[path = "tests/doctor.rs"]
mod tests;
