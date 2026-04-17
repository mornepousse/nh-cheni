//! `nixup check` command.
//!
//! Shows available updates for installed packages.
//! Compares local versions (from the nix store) with the latest
//! versions available on nixos-unstable (via Repology API).

use std::collections::HashMap;

use anyhow::Result;
use colored::Colorize;
use tracing::debug;

use crate::api::repology;
use crate::nix::{config, store};
use crate::version::compare::{compare_versions, VersionDiff};
use crate::version::parse::parse_version;

/// A package with its update status, ready for display.
struct CheckResult {
    name: String,
    installed: String,
    available: String,
    diff: VersionDiff,
}

/// Run the `nixup check` command.
///
/// If `category` is Some, only show packages from that module directory.
pub async fn run(category: Option<&str>) -> Result<()> {
    // 1. Detect the NixOS configuration
    let nix_config = config::detect()?;

    // 2. Get installed packages from the store
    let store_packages = store::read_installed_packages()?;
    let store_map: HashMap<String, String> = store_packages
        .iter()
        .map(|p| (p.name.to_lowercase(), p.version.clone()))
        .collect();

    // 3. Get package names from the config (all or filtered by category)
    let nix_files = match category {
        Some(cat) => {
            let files = config::list_module_files(&nix_config.flake_dir, cat);
            if files.is_empty() {
                anyhow::bail!(
                    "No module category '{}' found.\nAvailable: {}",
                    cat,
                    config::list_module_categories(&nix_config.flake_dir).join(", ")
                );
            }
            files
        }
        None => {
            // All .nix files from all module directories + home/
            let categories = config::list_module_categories(&nix_config.flake_dir);
            let mut files = Vec::new();
            for cat in &categories {
                files.extend(config::list_module_files(&nix_config.flake_dir, cat));
            }
            // Also scan home/ for home-manager packages
            let home_dir = nix_config.flake_dir.join("home");
            if home_dir.exists() {
                files.extend(config::list_module_files(
                    &nix_config.flake_dir.join("home").parent().unwrap_or(&nix_config.flake_dir),
                    "home",
                ));
            }
            files
        }
    };

    let config_names = config::extract_package_names(&nix_files);
    debug!("Config declares {} package names", config_names.len());

    // 4. Cross-reference: keep only packages that are both in config AND store
    let mut packages_to_check: Vec<(String, String)> = Vec::new();
    for name in &config_names {
        if let Some(version) = store_map.get(&name.to_lowercase()) {
            packages_to_check.push((name.clone(), version.clone()));
        }
    }

    if packages_to_check.is_empty() {
        println!("{}", "No packages found to check.".dimmed());
        return Ok(());
    }

    // 5. Query Repology for latest versions
    let names: Vec<String> = packages_to_check.iter().map(|(n, _)| n.clone()).collect();
    let header = match category {
        Some(cat) => format!("Checking {} packages (modules/{}/)", names.len(), cat),
        None => format!("Checking {} packages...", names.len()),
    };
    println!("{}\n", header.dimmed());

    let lookups = repology::lookup_versions(&names).await?;
    let lookup_map: HashMap<String, repology::PackageLookup> = lookups
        .into_iter()
        .map(|l| (l.name.clone(), l))
        .collect();

    // 6. Compare versions and build results
    let mut minor_updates = Vec::new();
    let mut major_updates = Vec::new();
    let mut up_to_date = 0;
    let mut newer = 0;
    let mut unknown = 0;

    for (name, installed_version) in &packages_to_check {
        let lookup = match lookup_map.get(name) {
            Some(l) => l,
            None => {
                unknown += 1;
                continue;
            }
        };

        let available = match &lookup.version {
            Some(v) => v,
            None => {
                unknown += 1;
                continue;
            }
        };

        let installed_parts = parse_version(installed_version);
        let available_parts = parse_version(available);
        let diff = compare_versions(&installed_parts, &available_parts);

        let result = CheckResult {
            name: name.clone(),
            installed: installed_version.clone(),
            available: available.clone(),
            diff: diff.clone(),
        };

        match diff {
            VersionDiff::Equal => up_to_date += 1,
            VersionDiff::Minor => minor_updates.push(result),
            VersionDiff::Major => major_updates.push(result),
            VersionDiff::Newer => newer += 1,
        }
    }

    // 7. Display results
    if !minor_updates.is_empty() {
        println!("{}:", "Updates available".yellow().bold());
        for r in &minor_updates {
            println!(
                "  {:<24} {:<14} {} {:<14} {}",
                r.name,
                r.installed.dimmed(),
                "→".dimmed(),
                r.available.green(),
                "(minor)".dimmed(),
            );
        }
        println!();
    }

    if !major_updates.is_empty() {
        println!(
            "{} {}:",
            "Major updates".red().bold(),
            "(use 'nixup pin --force' to apply)".dimmed(),
        );
        for r in &major_updates {
            println!(
                "  {:<24} {:<14} {} {:<14} {}",
                r.name,
                r.installed.dimmed(),
                "→".dimmed(),
                r.available.red(),
                "(major)".red(),
            );
        }
        println!();
    }

    if minor_updates.is_empty() && major_updates.is_empty() {
        println!("{}", "Everything is up to date!".green().bold());
        println!();
    }

    // Summary line
    let total = packages_to_check.len();
    println!(
        "{} {} | {} {} | {} {} | {} {} | {} {}",
        "Up to date:".dimmed(),
        up_to_date.to_string().green(),
        "Minor:".dimmed(),
        minor_updates.len().to_string().yellow(),
        "Major:".dimmed(),
        major_updates.len().to_string().red(),
        "Newer:".dimmed(),
        newer.to_string().cyan(),
        "Unknown:".dimmed(),
        unknown.to_string().dimmed(),
    );

    Ok(())
}
