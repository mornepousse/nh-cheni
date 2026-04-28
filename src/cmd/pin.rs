//! `cheni pin` and `cheni unpin` commands.
//!
//! Pin packages to nixpkgs-latest for granular updates.
//! When pinned, a package is pulled from the newer nixpkgs-latest input
//! instead of the regular nixpkgs.

use std::collections::HashMap;

use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;
use crate::nix::{config, flake, freezes, pins, store, version_cache};
use crate::nix::eval::lookup_or_eval;
use crate::nix::flake::{read_input_rev, target_system};
use crate::nix::version_cache::VersionCache;
use crate::version::compare::{compare_versions, VersionDiff};
use crate::version::parse::parse_version;

/// Run `cheni pin` with no arguments.
///
/// Lists the currently pinned packages with the store version each
/// one is currently providing, and flags pins that nixpkgs has
/// already caught up with (the pin is still technically honoured
/// but no longer routes the package anywhere different — `cheni clean`
/// would remove it).
pub fn list_pins() -> Result<()> {
    let nix_config = config::detect()?;
    let current_pins = pins::read(&nix_config.flake_dir)?;

    println!("{}\n", "=== cheni pin (list) ===".bold());

    if current_pins.is_empty() {
        println!("  {}", "no active pins.".dimmed());
        println!();
        println!("  Pin a package with '{}'.", "cheni pin <name>".bold());
        return Ok(());
    }

    let installed = store::read_installed_packages().unwrap_or_default();
    let lock_path = nix_config.flake_dir.join("flake.lock");
    let obsolete = super::obsolete::count_obsolete_pins(&lock_path, &current_pins);
    let all_obsolete = obsolete == current_pins.len() && obsolete > 0;

    println!(
        "  {} {} active{}",
        current_pins.len().to_string().bold(),
        crate::util::pluralize(current_pins.len(), "pin"),
        if all_obsolete {
            " (all obsolete — nixpkgs caught up)".yellow().to_string()
        } else if obsolete > 0 {
            format!(" ({} obsolete)", obsolete).yellow().to_string()
        } else {
            String::new()
        }
    );
    println!();

    for (idx, name) in current_pins.iter().enumerate() {
        let glyph = crate::util::tree_glyph(idx, current_pins.len());
        let version_display = installed
            .iter()
            .find(|p| p.name == *name)
            .map(|p| p.version.clone())
            .unwrap_or_else(|| "(not installed)".to_string());
        println!(
            "  {} {:<28} {}",
            glyph.dimmed(),
            name.bold(),
            version_display.dimmed()
        );
    }

    println!();
    if obsolete > 0 {
        println!(
            "  {} Run '{}' to drop obsolete pins.",
            "·".dimmed(),
            "cheni clean".bold()
        );
    }
    println!(
        "  {} Run '{}' to release one, or '{}' to release all.",
        "·".dimmed(),
        "cheni unpin <name>".bold(),
        "cheni unpin --all".bold()
    );
    Ok(())
}

/// Run `cheni pin <package>`.
///
/// Pins a single package by name.
pub async fn pin_one(name: &str, force: bool) -> Result<()> {
    let nix_config = config::detect()?;

    if !config::is_initialized(&nix_config.flake_dir) {
        super::check::print_first_run_hint();
        return Ok(());
    }

    // Flake inputs (e.g. zen-browser, claude-code) bypass the
    // nixpkgs-latest overlay — `nix flake update <name>` is the path.
    if flake::is_flake_input(&nix_config.flake_dir, name) {
        return pin_flake_input(&nix_config.flake_dir, name);
    }

    // Symmetric to `freeze_one`'s `reject_if_pinned` guard — a package
    // can't be pinned and frozen at once (the two overlays would race
    // on the same attr). Bail before touching anything.
    if freezes::read(&nix_config.flake_dir)?.contains_key(name) {
        anyhow::bail!(
            "{} is currently frozen — run `cheni unfreeze {}` first if you want to pin it instead.",
            name,
            name
        );
    }

    let store_pkg = store::find_by_name(name)?;
    let installed_version = store_pkg.version.clone();
    let available = lookup_available_version(&nix_config.flake_dir, name, &installed_version).await?;

    if !announce_pin_decision(name, &installed_version, available.as_deref(), force) {
        return Ok(());
    }

    // Educational contract block: show what the pin actually does
    // before the user commits to it. Default-yes prompt because the
    // user explicitly asked to pin — this is a sanity check, not a
    // safety gate.
    print_pin_contract(name, &installed_version);
    if !confirm(
        &format!("Pin {} at {}?", name, installed_version),
        true,
    )? {
        println!("{}", "  Cancelled — nothing pinned.".yellow());
        return Ok(());
    }

    let added = pins::add(&nix_config.flake_dir, &[name.to_string()])?;
    if added.is_empty() {
        println!("{} was already pinned.", name);
    } else {
        println!("\n{} Pinned {}.", "✓".green(), name.bold());
    }
    println!("Run '{}' to apply.", "cheni build".bold());
    Ok(())
}

/// Print the "what this pin means" block. Kept short on purpose —
/// three lines of effect, one line of how to undo.
///
/// NB: a cheni "pin" is NOT a version freeze. It routes the package
/// through the `nixpkgs-latest` input instead of `nixpkgs`, so the
/// package typically gets a newer version and continues to track
/// that input's bumps at every `cheni upgrade`. When `nixpkgs`
/// catches up, the pin becomes obsolete and `cheni clean` drops it.
fn print_pin_contract(name: &str, installed: &str) {
    println!();
    println!("  {}", "What this does:".bold());
    println!(
        "    Routes {} through nixpkgs-latest (currently {} in the store).",
        name.bold(),
        installed.dimmed()
    );
    println!(
        "    Next '{}' will bump {} along with nixpkgs-latest,",
        "cheni upgrade".bold(),
        name
    );
    println!("    typically to a newer version than plain nixpkgs provides.");
    println!(
        "    The pin auto-expires once nixpkgs catches up (via '{}').",
        "cheni clean".bold()
    );
    println!(
        "    Undo immediately with '{}' to go back to nixpkgs.",
        format!("cheni unpin {}", name).bold()
    );
    println!();
}

// `find_in_store` was removed — call `crate::nix::store::find_by_name`
// directly (shared with `cmd::freeze`).

/// Look up the available version of a single package by evaluating
/// nixpkgs-latest at its current locked rev.
///
/// `_installed` is kept in the signature for future disambiguation
/// (e.g. kdePackages / libsForQt5 namespace collisions) but is
/// unused in the nix-eval path.
async fn lookup_available_version(
    flake_dir: &Path,
    name: &str,
    _installed: &str,
) -> Result<Option<String>> {
    let Some(rev) = read_input_rev(flake_dir, "nixpkgs-latest") else {
        return Ok(None);
    };
    let attr = format!("legacyPackages.{}.{}", target_system(), name);
    let mut cache = VersionCache::load(&version_cache::cache_path())?;
    let result = lookup_or_eval(&mut cache, "nixpkgs-latest", &rev, &attr)?;
    cache.save(&version_cache::cache_path())?;
    Ok(result)
}

/// Print the version transition + decide whether to proceed with the pin.
/// Returns `true` when the caller should add the pin, `false` when we
/// already short-circuited (already up to date, newer, blocked major).
fn announce_pin_decision(
    name: &str,
    installed: &str,
    available: Option<&str>,
    force: bool,
) -> bool {
    let Some(available) = available else {
        println!(
            "{}: {} → {} (version unknown, pinning anyway)",
            name,
            installed.dimmed(),
            "?".dimmed()
        );
        return true;
    };

    let diff = compare_versions(&parse_version(installed), &parse_version(available));
    match diff {
        VersionDiff::Equal => {
            println!("{} is already up to date ({})", name, installed);
            false
        }
        VersionDiff::Newer => {
            println!(
                "{} is already newer than nixpkgs ({} > {})",
                name, installed, available
            );
            false
        }
        VersionDiff::Major if !force => {
            println!(
                "{}: {} → {} ({})",
                name,
                installed.dimmed(),
                available.red(),
                "major update".red()
            );
            println!(
                "\n{}",
                "This is a major version bump. Use --force to pin anyway.".yellow()
            );
            false
        }
        VersionDiff::Major => {
            println!(
                "{}: {} → {} ({})",
                name,
                installed.dimmed(),
                available.green(),
                "major".red()
            );
            true
        }
        VersionDiff::Minor => {
            println!(
                "{}: {} → {} ({})",
                name,
                installed.dimmed(),
                available.green(),
                "minor".yellow()
            );
            true
        }
    }
}

/// Run `cheni pin --<category>`.
///
/// Pins all packages from a module category that have minor updates.
/// Major updates are shown separately and require confirmation.
pub async fn pin_category(category: &str, force: bool) -> Result<()> {
    let nix_config = config::detect()?;

    let to_check = installed_packages_in_category(&nix_config.flake_dir, category)?;
    if to_check.is_empty() {
        println!("No installed packages found in modules/{}/.", category);
        println!(
            "  {} categories: {}",
            "·".dimmed(),
            config::list_module_categories(&nix_config.flake_dir).join(", ")
        );
        return Ok(());
    }

    println!(
        "{}",
        format!(
            "Checking {} packages in modules/{}/...\n",
            to_check.len(),
            category
        )
        .dimmed()
    );

    let (minor_updates, major_updates) = classify_pin_targets(&nix_config.flake_dir, &to_check).await?;
    if minor_updates.is_empty() && major_updates.is_empty() {
        println!("{} Everything is up to date.", "✓".green());
        return Ok(());
    }

    let mut to_pin: Vec<String> = Vec::new();
    to_pin.extend(confirm_pin_block(
        &minor_updates,
        UpdateKind::Minor,
        force,
    )?);
    to_pin.extend(confirm_pin_block(
        &major_updates,
        UpdateKind::Major,
        force,
    )?);

    if to_pin.is_empty() {
        println!("\n{} Nothing pinned.", "·".dimmed());
        return Ok(());
    }
    let added = pins::add(&nix_config.flake_dir, &to_pin)?;
    println!(
        "\n{} Pinned {} {}.",
        "✓".green(),
        added.len().to_string().bold(),
        crate::util::pluralize(added.len(), "package")
    );
    println!("Run '{}' to apply.", "cheni build".bold());
    Ok(())
}

/// Cross-reference the packages declared in a module category with what's
/// actually in the store. Returns (name, installed_version) pairs.
fn installed_packages_in_category(
    flake_dir: &std::path::Path,
    category: &str,
) -> Result<Vec<(String, String)>> {
    let nix_files = config::list_module_files(flake_dir, category);
    if nix_files.is_empty() {
        anyhow::bail!(
            "No module category '{}' found.\nAvailable: {}",
            category,
            config::list_module_categories(flake_dir).join(", ")
        );
    }

    let config_names = config::extract_package_names(&nix_files);
    let store_packages = store::read_installed_packages()?;
    let store_map: HashMap<String, String> = store_packages
        .iter()
        .map(|p| (p.name.to_lowercase(), p.version.clone()))
        .collect();

    // Frozen packages must not become candidates for `cheni pin --<cat>`
    // — pin and freeze are mutually exclusive on a given attr (they
    // are two overlays writing the same name).
    let frozen = freezes::read(flake_dir)?;

    Ok(config_names
        .into_iter()
        .filter(|name| !frozen.contains_key(name))
        .filter_map(|name| {
            store_map
                .get(&name.to_lowercase())
                .map(|v| (name, v.clone()))
        })
        .collect())
}

/// One `(name, installed, available)` row, grouped by update kind.
type PinRow = (String, String, String);

/// Evaluate nixpkgs-latest for each package in `to_check` and bucket
/// each result into Minor or Major.
///
/// Packages with no version in nixpkgs-latest (eval returns None) or
/// whose installed version is already ≥ available are silently skipped.
async fn classify_pin_targets(
    flake_dir: &Path,
    to_check: &[(String, String)],
) -> Result<(Vec<PinRow>, Vec<PinRow>)> {
    let cache_path = version_cache::cache_path();
    let mut cache = VersionCache::load(&cache_path)?;
    let rev = read_input_rev(flake_dir, "nixpkgs-latest");
    if rev.is_none() {
        eprintln!(
            "  Note: input 'nixpkgs-latest' is not configured in flake.lock, \
             so version targets are unknown. Add the input or ignore this note."
        );
    }

    let mut minor = Vec::new();
    let mut major = Vec::new();

    for (name, installed) in to_check {
        let Some(ref rev) = rev else { continue; };
        let attr = format!("legacyPackages.{}.{}", target_system(), name);
        let Some(available) = lookup_or_eval(&mut cache, "nixpkgs-latest", rev, &attr)? else {
            continue;
        };
        match compare_versions(&parse_version(installed), &parse_version(&available)) {
            VersionDiff::Minor => minor.push((name.clone(), installed.clone(), available)),
            VersionDiff::Major => major.push((name.clone(), installed.clone(), available)),
            _ => {}
        }
    }

    cache.save(&cache_path)?;
    Ok((minor, major))
}

/// What kind of update batch this is — controls colour, default
/// confirmation answer, and whether `--force` is required.
#[derive(Clone, Copy)]
enum UpdateKind {
    Minor,
    Major,
}

/// Print the batch and either gather names to pin (Minor: default-yes,
/// Major: requires --force) or skip with a hint. Returns the names the
/// user agreed to pin from this batch.
fn confirm_pin_block(
    updates: &[PinRow],
    kind: UpdateKind,
    force: bool,
) -> Result<Vec<String>> {
    if updates.is_empty() {
        return Ok(Vec::new());
    }
    match kind {
        UpdateKind::Minor => println!("{}:", "Minor updates (safe)".yellow().bold()),
        UpdateKind::Major => println!(
            "\n{}:",
            "Major updates (breaking changes possible)".red().bold()
        ),
    }
    for (name, installed, available) in updates {
        let new_ver = match kind {
            UpdateKind::Minor => available.green(),
            UpdateKind::Major => available.red(),
        };
        println!(
            "  {:<24} {:<14} {} {}",
            name,
            installed.dimmed(),
            "→".dimmed(),
            new_ver
        );
    }
    println!();

    let prompt = match kind {
        UpdateKind::Minor => format!("Pin {} minor {}?", updates.len(), crate::util::pluralize(updates.len(), "update")),
        UpdateKind::Major => format!("Pin {} major {}?", updates.len(), crate::util::pluralize(updates.len(), "update")),
    };
    let default_yes = matches!(kind, UpdateKind::Minor);

    let proceed = match kind {
        UpdateKind::Minor => true,
        UpdateKind::Major if force => true,
        UpdateKind::Major => {
            println!("{}", "Use --force to allow pinning major updates.".dimmed());
            false
        }
    };
    if !proceed {
        return Ok(Vec::new());
    }

    if !confirm(&prompt, default_yes)? {
        return Ok(Vec::new());
    }
    Ok(updates.iter().map(|(name, _, _)| name.clone()).collect())
}

/// Run `cheni unpin <package>`.
pub fn unpin_one(name: &str, yes: bool) -> Result<()> {
    let nix_config = config::detect()?;
    let current_pins = pins::read(&nix_config.flake_dir)?;

    if !current_pins.iter().any(|p| p == name) {
        println!("'{}' was not pinned.", name);
        return Ok(());
    }

    let installed = store::read_installed_packages().unwrap_or_default();
    let version = installed
        .iter()
        .find(|p| p.name == name)
        .map(|p| p.version.clone())
        .unwrap_or_else(|| "(unknown)".to_string());

    println!("{}\n", "=== cheni unpin ===".bold());
    println!(
        "  {} is currently routed through nixpkgs-latest (store version {}).",
        name.bold(),
        version.dimmed()
    );
    println!(
        "  Unpinning routes {} back through plain nixpkgs, which usually",
        name
    );
    println!(
        "  provides an older version. Next '{}' will apply that move.",
        "cheni upgrade".bold()
    );
    println!();

    if !yes && !confirm(&format!("Unpin {}?", name), false)? {
        println!("{}", "  Cancelled — pin kept.".yellow());
        return Ok(());
    }

    let removed = pins::remove(&nix_config.flake_dir, &[name.to_string()])?;
    if removed.is_empty() {
        // Race condition: pin disappeared between the read and the remove.
        println!("'{}' was not pinned.", name);
    } else {
        println!("{} Unpinned {}.", "✓".green(), name.bold());
        println!("Run '{}' to apply.", "cheni build".bold());
    }
    Ok(())
}

/// Run `cheni unpin --all`.
pub fn unpin_all(yes: bool) -> Result<()> {
    let nix_config = config::detect()?;
    let current_pins = pins::read(&nix_config.flake_dir)?;

    if current_pins.is_empty() {
        println!("{} No pins to remove.", "✓".green());
        return Ok(());
    }

    let installed = store::read_installed_packages().unwrap_or_default();

    println!("{}\n", "=== cheni unpin ===".bold());
    println!(
        "  This will release {} {}:",
        current_pins.len().to_string().bold(),
        crate::util::pluralize(current_pins.len(), "pin")
    );
    for (idx, name) in current_pins.iter().enumerate() {
        let glyph = crate::util::tree_glyph(idx, current_pins.len());
        let version = installed
            .iter()
            .find(|p| p.name == *name)
            .map(|p| p.version.clone())
            .unwrap_or_else(|| "(unknown)".to_string());
        println!(
            "    {} {:<28} {}",
            glyph.dimmed(),
            name.bold(),
            version.dimmed()
        );
    }
    println!();
    println!(
        "  All of these will be routed back through plain nixpkgs. Next '{}'",
        "cheni upgrade".bold()
    );
    println!("  will usually move them to an older version (the nixpkgs baseline).");
    println!();

    if !yes
        && !confirm(
            &format!("Unpin all {}?", crate::util::count_phrase(current_pins.len(), "pin")),
            false,
        )?
    {
        println!("{}", "  Cancelled — pins kept.".yellow());
        return Ok(());
    }

    let count = pins::clear(&nix_config.flake_dir)?;
    println!("{} Removed {} {}.", "✓".green(), count.to_string().bold(), crate::util::pluralize(count, "pin"));
    println!("Run '{}' to apply.", "cheni build".bold());
    Ok(())
}

// `confirm` was removed — call `crate::util::confirm` directly.
use crate::util::confirm;

/// Run `cheni pin --flakes`.
///
/// Shows all flake inputs with available updates and offers
/// to update them via `nix flake update <input>`.
pub async fn pin_flake_inputs() -> Result<()> {
    let nix_config = config::detect()?;
    let mut inputs = flake::read_flake_inputs(&nix_config.flake_dir)
        .context("Failed to read flake inputs")?;
    if inputs.is_empty() {
        println!(
            "{}",
            "No flake inputs found (besides infrastructure).".dimmed()
        );
        return Ok(());
    }

    println!("{}", "Checking flake inputs for updates...\n".dimmed());
    flake::check_flake_updates(&mut inputs);

    let (with_updates, up_to_date, unknown) = partition_flake_inputs(&inputs);
    print_flake_status_lines(&up_to_date, "ok".green().to_string());
    print_flake_status_lines(&unknown, "? (could not check)".dimmed().to_string());

    if with_updates.is_empty() {
        println!("\n{}", "All flake inputs are up to date!".green());
        return Ok(());
    }

    print_flake_updates_block(&with_updates);
    if !confirm(
        &format!("Update {} flake {}?", with_updates.len(), crate::util::pluralize(with_updates.len(), "input")),
        true,
    )? {
        println!("No flake inputs updated.");
        return Ok(());
    }
    let updated = apply_flake_updates(&nix_config.flake_dir, &with_updates);
    println!(
        "\n{} Updated {} flake {}.",
        "✓".green(),
        updated.to_string().bold(),
        crate::util::pluralize(updated, "input")
    );
    println!("Run '{}' to rebuild.", "cheni build".bold());
    Ok(())
}

/// Split flake inputs into three buckets based on their `has_update` flag.
fn partition_flake_inputs(
    inputs: &[flake::FlakeInput],
) -> (Vec<&flake::FlakeInput>, Vec<&flake::FlakeInput>, Vec<&flake::FlakeInput>) {
    let with_updates: Vec<_> = inputs.iter().filter(|i| i.has_update == Some(true)).collect();
    let up_to_date: Vec<_> = inputs.iter().filter(|i| i.has_update == Some(false)).collect();
    let unknown: Vec<_> = inputs.iter().filter(|i| i.has_update.is_none()).collect();
    (with_updates, up_to_date, unknown)
}

fn print_flake_status_lines(inputs: &[&flake::FlakeInput], status: String) {
    for input in inputs {
        let version = input.installed_version.as_deref().unwrap_or("?");
        println!(
            "  {:<24} {:<14} {}",
            input.name,
            version.dimmed(),
            status,
        );
    }
}

fn print_flake_updates_block(inputs: &[&flake::FlakeInput]) {
    println!("\n{}:", "Flake inputs with updates".yellow().bold());
    for input in inputs {
        let version = input.installed_version.as_deref().unwrap_or("?");
        let age = match &input.remote_age {
            Some(date) => format!("({date})"),
            None => String::new(),
        };
        println!(
            "  {:<24} {:<14} {} {}",
            input.name,
            version.dimmed(),
            "UPDATE".yellow(),
            age.dimmed(),
        );
    }
    println!();
}

/// Run `nix flake update <name>` for each input in turn. Errors are
/// reported per-input and don't stop the rest. Returns the count that
/// updated successfully.
fn apply_flake_updates(flake_dir: &std::path::Path, inputs: &[&flake::FlakeInput]) -> usize {
    let mut updated = 0;
    for input in inputs {
        println!("\n{} {}...", "Updating".dimmed(), input.name.bold());
        match pin_flake_input(flake_dir, &input.name) {
            Ok(()) => updated += 1,
            Err(e) => {
                println!("{} Failed to update {}: {}", "✗".red(), input.name, e);
            }
        }
    }
    updated
}

/// Pin a flake input by updating it directly.
///
/// Instead of using the nixpkgs-latest overlay, this runs
/// `nix flake update <input-name>` to fetch the latest version.
fn pin_flake_input(flake_dir: &std::path::Path, name: &str) -> Result<()> {
    println!(
        "{} is a flake input — updating directly.\n",
        name.bold()
    );

    let status = std::process::Command::new("nix")
        .args(["flake", "update", "--", name])
        .current_dir(flake_dir)
        .status()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;

    if !status.success() {
        anyhow::bail!("nix flake update {} failed", name);
    }

    println!("\n{} Updated flake input {}.", "✓".green(), name.bold());
    println!("Run '{}' to rebuild.", "cheni build".bold());
    Ok(())
}

