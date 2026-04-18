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

    // Check if this is a flake input (e.g. zen-browser, claude-code)
    if flake::is_flake_input(&nix_config.flake_dir, name) {
        return pin_flake_input(&nix_config.flake_dir, name);
    }

    // Check if the package exists in the store
    let store_packages = store::read_installed_packages()?;
    let store_pkg = match store_packages
        .iter()
        .find(|p| p.name.to_lowercase() == name.to_lowercase())
    {
        Some(p) => p,
        None => {
            anyhow::bail!(
                "Package '{}' not found in the nix store.\nIs it installed?",
                name
            );
        }
    };
    let installed_version = &store_pkg.version;

    // Check the available version on Repology
    let lookups = repology::lookup_versions(&[name.to_string()]).await?;
    let lookup = lookups.first();

    match lookup.and_then(|l| l.version.as_ref()) {
        Some(available) => {
            let installed_parts = parse_version(installed_version);
            let available_parts = parse_version(available);
            let diff = compare_versions(&installed_parts, &available_parts);

            match diff {
                VersionDiff::Equal => {
                    println!("{} is already up to date ({})", name, installed_version);
                    return Ok(());
                }
                VersionDiff::Newer => {
                    println!(
                        "{} is already newer than nixpkgs ({} > {})",
                        name, installed_version, available
                    );
                    return Ok(());
                }
                VersionDiff::Major if !force => {
                    println!(
                        "{}: {} → {} ({})",
                        name,
                        installed_version.dimmed(),
                        available.red(),
                        "major update".red()
                    );
                    println!(
                        "\n{}",
                        "This is a major version bump. Use --force to pin anyway.".yellow()
                    );
                    return Ok(());
                }
                _ => {
                    let label = match diff {
                        VersionDiff::Major => "major".red().to_string(),
                        _ => "minor".yellow().to_string(),
                    };
                    println!(
                        "{}: {} → {} ({})",
                        name, installed_version.dimmed(), available.green(), label
                    );
                }
            }
        }
        None => {
            println!(
                "{}: {} → {} (version unknown, pinning anyway)",
                name,
                installed_version.dimmed(),
                "?".dimmed()
            );
        }
    }

    // Add to pins
    let added = pins::add(&nix_config.flake_dir, &[name.to_string()])?;
    if added.is_empty() {
        println!("{} was already pinned.", name);
    } else {
        println!("\n{} Pinned {}.", "✓".green(), name.bold());
    }

    println!("Run '{}' to apply.", "cheni build".bold());
    Ok(())
}

/// Run `cheni pin --<category>`.
///
/// Pins all packages from a module category that have minor updates.
/// Major updates are shown separately and require confirmation.
pub async fn pin_category(category: &str, force: bool) -> Result<()> {
    let nix_config = config::detect()?;

    // Get packages from this category
    let nix_files = config::list_module_files(&nix_config.flake_dir, category);
    if nix_files.is_empty() {
        anyhow::bail!(
            "No module category '{}' found.\nAvailable: {}",
            category,
            config::list_module_categories(&nix_config.flake_dir).join(", ")
        );
    }

    let config_names = config::extract_package_names(&nix_files);
    let store_packages = store::read_installed_packages()?;
    let store_map: HashMap<String, String> = store_packages
        .iter()
        .map(|p| (p.name.to_lowercase(), p.version.clone()))
        .collect();

    // Keep only installed packages from this category
    let mut to_check: Vec<(String, String)> = Vec::new();
    for name in &config_names {
        if let Some(version) = store_map.get(&name.to_lowercase()) {
            to_check.push((name.clone(), version.clone()));
        }
    }

    if to_check.is_empty() {
        println!("No installed packages found in modules/{}/.", category);
        return Ok(());
    }

    // Query Repology
    let names: Vec<String> = to_check.iter().map(|(n, _)| n.clone()).collect();
    println!(
        "{}",
        format!("Checking {} packages in modules/{}/...\n", names.len(), category).dimmed()
    );

    let lookups = repology::lookup_versions(&names).await?;
    let lookup_map: HashMap<String, _> = lookups
        .into_iter()
        .map(|l| (l.name.clone(), l))
        .collect();

    // Separate minor and major updates
    let mut minor_updates: Vec<(String, String, String)> = Vec::new();
    let mut major_updates: Vec<(String, String, String)> = Vec::new();

    for (name, installed) in &to_check {
        let available = match lookup_map.get(name).and_then(|l| l.version.as_ref()) {
            Some(v) => v,
            None => continue,
        };

        let installed_parts = parse_version(installed);
        let available_parts = parse_version(available);

        match compare_versions(&installed_parts, &available_parts) {
            VersionDiff::Minor => {
                minor_updates.push((name.clone(), installed.clone(), available.clone()));
            }
            VersionDiff::Major => {
                major_updates.push((name.clone(), installed.clone(), available.clone()));
            }
            _ => {}
        }
    }

    if minor_updates.is_empty() && major_updates.is_empty() {
        println!("{}", "Everything is up to date!".green());
        return Ok(());
    }

    let mut to_pin: Vec<String> = Vec::new();

    // Minor updates — grouped confirmation
    if !minor_updates.is_empty() {
        println!("{}:", "Minor updates (safe)".yellow().bold());
        for (name, installed, available) in &minor_updates {
            println!(
                "  {:<24} {:<14} {} {}",
                name, installed.dimmed(), "→".dimmed(), available.green()
            );
        }
        println!();

        if confirm(&format!("Pin {} minor update(s)?", minor_updates.len()), true)? {
            for (name, _, _) in &minor_updates {
                to_pin.push(name.clone());
            }
        }
    }

    // Major updates — grouped confirmation (default no)
    if !major_updates.is_empty() {
        println!("\n{}:", "Major updates (breaking changes possible)".red().bold());
        for (name, installed, available) in &major_updates {
            println!(
                "  {:<24} {:<14} {} {}",
                name, installed.dimmed(), "→".dimmed(), available.red()
            );
        }
        println!();

        if force {
            if confirm(&format!("Pin {} major update(s)?", major_updates.len()), false)? {
                for (name, _, _) in &major_updates {
                    to_pin.push(name.clone());
                }
            }
        } else {
            println!("{}", "Use --force to allow pinning major updates.".dimmed());
        }
    }

    // Apply pins
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

    // Lire les flake inputs depuis flake.lock
    let mut inputs = flake::read_flake_inputs(&nix_config.flake_dir)
        .context("Failed to read flake inputs")?;

    if inputs.is_empty() {
        println!("{}", "No flake inputs found (besides infrastructure).".dimmed());
        return Ok(());
    }

    // Vérifier les mises à jour disponibles via GitHub/GitLab API
    println!("{}", "Checking flake inputs for updates...\n".dimmed());
    flake::check_flake_updates(&mut inputs);

    // Séparer les inputs avec et sans mise à jour
    let inputs_with_updates: Vec<&flake::FlakeInput> = inputs.iter()
        .filter(|i| i.has_update == Some(true))
        .collect();

    let inputs_up_to_date: Vec<&flake::FlakeInput> = inputs.iter()
        .filter(|i| i.has_update == Some(false))
        .collect();

    let inputs_unknown: Vec<&flake::FlakeInput> = inputs.iter()
        .filter(|i| i.has_update.is_none())
        .collect();

    // Afficher les inputs à jour
    if !inputs_up_to_date.is_empty() {
        for input in &inputs_up_to_date {
            let version_str = input.installed_version.as_deref().unwrap_or("?");
            println!(
                "  {:<24} {:<14} {}",
                input.name,
                version_str.dimmed(),
                "ok".green(),
            );
        }
    }

    // Afficher les inputs dont le statut est inconnu
    if !inputs_unknown.is_empty() {
        for input in &inputs_unknown {
            let version_str = input.installed_version.as_deref().unwrap_or("?");
            println!(
                "  {:<24} {:<14} {}",
                input.name,
                version_str.dimmed(),
                "? (could not check)".dimmed(),
            );
        }
    }

    // Si rien à mettre à jour, on s'arrête
    if inputs_with_updates.is_empty() {
        println!("\n{}", "All flake inputs are up to date!".green());
        return Ok(());
    }

    // Afficher les inputs avec mise à jour disponible
    println!("\n{}:", "Flake inputs with updates".yellow().bold());
    for input in &inputs_with_updates {
        let version_str = input.installed_version.as_deref().unwrap_or("?");
        let age_info = match &input.remote_age {
            Some(date) => format!("({date})"),
            None => String::new(),
        };
        println!(
            "  {:<24} {:<14} {} {}",
            input.name,
            version_str.dimmed(),
            "UPDATE".yellow(),
            age_info.dimmed(),
        );
    }
    println!();

    // Demander confirmation
    let count = inputs_with_updates.len();
    if !confirm(&format!("Update {} flake input(s)?", count), true)? {
        println!("No flake inputs updated.");
        return Ok(());
    }

    // Mettre à jour chaque input
    let mut updated = 0;
    for input in &inputs_with_updates {
        println!("\n{} {}...", "Updating".dimmed(), input.name.bold());
        let result = pin_flake_input(&nix_config.flake_dir, &input.name);
        match result {
            Ok(()) => updated += 1,
            Err(e) => {
                println!("{} Failed to update {}: {}", "!".red(), input.name, e);
            }
        }
    }

    println!(
        "\n{} Updated {} flake input(s).",
        "✓".green(),
        updated.to_string().bold(),
    );
    println!("Run '{}' to rebuild.", "cheni build".bold());

    Ok(())
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

