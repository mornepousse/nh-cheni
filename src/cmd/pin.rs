//! `cheni pin` and `cheni unpin` commands.
//!
//! Pin packages to nixpkgs-latest for granular updates.
//! When pinned, a package is pulled from the newer nixpkgs-latest input
//! instead of the regular nixpkgs.

use std::collections::HashMap;
use std::io::{self, Write};

use anyhow::{Context, Result};
use colored::Colorize;
use crate::api::repology;
use crate::nix::{config, flake, pins, store};
use crate::version::compare::{compare_versions, VersionDiff};
use crate::version::parse::parse_version;

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

    let store_pkg = find_in_store(name)?;
    let installed_version = store_pkg.version.clone();
    let available = lookup_available_version(name, &installed_version).await?;

    if !announce_pin_decision(name, &installed_version, available.as_deref(), force) {
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

/// Locate a package in the nix store by case-insensitive name.
fn find_in_store(name: &str) -> Result<store::StorePackage> {
    let store_packages = store::read_installed_packages()?;
    store_packages
        .into_iter()
        .find(|p| p.name.to_lowercase() == name.to_lowercase())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Package '{}' not found in the nix store.\nIs it installed?",
                name
            )
        })
}

/// Look up the latest version of a single package on Repology, passing
/// the installed version as a disambiguation hint.
async fn lookup_available_version(name: &str, installed: &str) -> Result<Option<String>> {
    let lookups =
        repology::lookup_versions(&[(name.to_string(), Some(installed.to_string()))]).await?;
    Ok(lookups
        .into_iter()
        .next()
        .and_then(|l| l.version))
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

    let (minor_updates, major_updates) = classify_pin_targets(&to_check).await?;
    if minor_updates.is_empty() && major_updates.is_empty() {
        println!("{}", "Everything is up to date!".green());
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
        println!("\nNo packages pinned.");
        return Ok(());
    }
    let added = pins::add(&nix_config.flake_dir, &to_pin)?;
    println!(
        "\n{} Pinned {} package(s).",
        "✓".green(),
        added.len().to_string().bold()
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

    Ok(config_names
        .into_iter()
        .filter_map(|name| {
            store_map
                .get(&name.to_lowercase())
                .map(|v| (name, v.clone()))
        })
        .collect())
}

/// One `(name, installed, available)` row, grouped by update kind.
type PinRow = (String, String, String);

/// Run Repology lookups with installed-version hints (disambiguates
/// kdePackages / libsForQt5 / xfce4-* namespace collisions) and bucket
/// each package into Minor or Major.
async fn classify_pin_targets(
    to_check: &[(String, String)],
) -> Result<(Vec<PinRow>, Vec<PinRow>)> {
    let packages: Vec<(String, Option<String>)> = to_check
        .iter()
        .map(|(n, v)| (n.clone(), Some(v.clone())))
        .collect();
    let lookups = repology::lookup_versions(&packages).await?;
    let lookup_map: HashMap<String, _> = lookups
        .into_iter()
        .map(|l| (l.name.clone(), l))
        .collect();

    let mut minor = Vec::new();
    let mut major = Vec::new();
    for (name, installed) in to_check {
        let Some(available) = lookup_map.get(name).and_then(|l| l.version.as_ref()) else {
            continue;
        };
        match compare_versions(&parse_version(installed), &parse_version(available)) {
            VersionDiff::Minor => minor.push((name.clone(), installed.clone(), available.clone())),
            VersionDiff::Major => major.push((name.clone(), installed.clone(), available.clone())),
            _ => {}
        }
    }
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
        UpdateKind::Minor => format!("Pin {} minor update(s)?", updates.len()),
        UpdateKind::Major => format!("Pin {} major update(s)?", updates.len()),
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
pub fn unpin_one(name: &str) -> Result<()> {
    let nix_config = config::detect()?;

    let removed = pins::remove(&nix_config.flake_dir, &[name.to_string()])?;

    if removed.is_empty() {
        println!("'{}' was not pinned.", name);
    } else {
        println!("{} Unpinned {}.", "✓".green(), name.bold());
        println!("Run '{}' to apply.", "cheni build".bold());
    }

    Ok(())
}

/// Run `cheni unpin --all`.
pub fn unpin_all() -> Result<()> {
    let nix_config = config::detect()?;

    let count = pins::clear(&nix_config.flake_dir)?;

    if count == 0 {
        println!("No pins to remove.");
    } else {
        println!("{} Removed {} pin(s).", "✓".green(), count.to_string().bold());
        println!("Run '{}' to apply.", "cheni build".bold());
    }

    Ok(())
}

/// Ask a yes/no question. Returns true for yes.
fn confirm(question: &str, default_yes: bool) -> Result<bool> {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{} {} ", question, hint.dimmed());
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();

    if answer.is_empty() {
        return Ok(default_yes);
    }

    Ok(answer == "y" || answer == "yes")
}

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
        &format!("Update {} flake input(s)?", with_updates.len()),
        true,
    )? {
        println!("No flake inputs updated.");
        return Ok(());
    }
    let updated = apply_flake_updates(&nix_config.flake_dir, &with_updates);
    println!(
        "\n{} Updated {} flake input(s).",
        "✓".green(),
        updated.to_string().bold(),
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
                println!("{} Failed to update {}: {}", "!".red(), input.name, e);
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
        .args(["flake", "update", name])
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

