//! `cheni check` command.
//!
//! Shows available updates for installed packages.
//! Compares local versions (from the nix store) with the latest
//! versions available on nixos-unstable (via Repology API).

use std::collections::HashMap;

use anyhow::Result;
use colored::Colorize;
use tracing::debug;

use crate::api::repology;
use crate::nix::{config, flake, pins, store};
use crate::version::compare::{compare_versions, VersionDiff};
use crate::version::parse::parse_version;

use super::obsolete::count_obsolete_pins;

/// A package with its update status, ready for display.
struct CheckResult {
    name: String,
    installed: String,
    available: String,
    /// Short relative path to the .nix file that declares this package
    /// (e.g. "modules/dev/esp-idf.nix"). None if we couldn't trace it back.
    declared_in: Option<String>,
}

/// Run the `cheni check` command.
///
/// If `category` is Some, only show packages from that module directory.
pub async fn run(category: Option<&str>, details: bool) -> Result<()> {
    // 1. Detect the NixOS configuration
    let nix_config = config::detect()?;

    if !config::is_initialized(&nix_config.flake_dir) {
        print_first_run_hint();
        return Ok(());
    }

    // Automatic warning if any pins are obsolete
    let current_pins = pins::read(&nix_config.flake_dir)?;
    if !current_pins.is_empty() {
        let lock_path = nix_config.flake_dir.join("flake.lock");
        let obsolete = count_obsolete_pins(&lock_path, &current_pins);
        if obsolete > 0 {
            println!(
                "{} {} obsolete pin(s) detected. Run '{}' to remove.\n",
                "Note:".yellow(),
                obsolete,
                "cheni clean".bold()
            );
        }
    }

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
                let home_parent = nix_config.flake_dir.join("home");
                let base_dir = home_parent.parent().unwrap_or(&nix_config.flake_dir);
                files.extend(config::list_module_files(base_dir, "home"));
            }
            files
        }
    };

    // Limit "in <file>" attribution to files that are actually imported
    // by the active host configuration. Without this filter, packages
    // declared in commented-out or otherwise inactive modules would be
    // misreported as the source.
    let active_set: Option<std::collections::HashSet<std::path::PathBuf>> =
        if category.is_some() {
            // User explicitly asked for a category — show everything in it.
            None
        } else {
            config::list_active_modules(&nix_config.flake_dir, &nix_config.hostname)
                .map(|v| v.into_iter().collect())
        };

    let mut names_with_files = config::extract_package_names_with_files(&nix_files);
    if let Some(active) = &active_set {
        for paths in names_with_files.values_mut() {
            paths.retain(|p| {
                p.canonicalize()
                    .ok()
                    .map(|c| active.contains(&c))
                    .unwrap_or(false)
            });
        }
        // Drop entries whose only declarations were in inactive files.
        names_with_files.retain(|_, paths| !paths.is_empty());
    }
    debug!("Config declares {} package names (active filter applied: {})",
        names_with_files.len(), active_set.is_some());

    // 4. Cross-reference: keep only packages that are both in config AND store
    let mut packages_to_check: Vec<(String, String)> = Vec::new();
    let mut sorted_names: Vec<&String> = names_with_files.keys().collect();
    sorted_names.sort();
    for name in &sorted_names {
        if let Some(version) = store_map.get(&name.to_lowercase()) {
            packages_to_check.push(((*name).clone(), version.clone()));
        }
    }

    if packages_to_check.is_empty() {
        println!("{}", "No packages found to check.".dimmed());
        return Ok(());
    }

    // 5. Query Repology for package versions AND flake updates concurrently.
    let names: Vec<String> = packages_to_check.iter().map(|(n, _)| n.clone()).collect();
    let header = match category {
        Some(cat) => format!("Checking {} packages (modules/{}/) + flake inputs", names.len(), cat),
        None => format!("Checking {} packages + flake inputs...", names.len()),
    };
    println!("{}", header.dimmed());

    // Spinner in background while API calls are running. Skip if stderr is
    // not a TTY (piped/redirected) — \r artifacts would clutter the output.
    use std::io::IsTerminal;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let done = Arc::new(AtomicBool::new(false));
    let done_clone = done.clone();
    let is_tty = std::io::stderr().is_terminal();
    let spinner = std::thread::spawn(move || {
        if !is_tty {
            return;
        }
        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let mut i = 0;
        while !done_clone.load(Ordering::Relaxed) {
            eprint!("\r  {} Querying remote APIs...", frames[i % frames.len()]);
            std::thread::sleep(std::time::Duration::from_millis(100));
            i += 1;
        }
        eprint!("\r                              \r");
    });

    // Spawn flake check in a blocking thread (uses sync reqwest + std::thread::scope).
    // It runs in parallel with the async Repology lookups below.
    let flake_dir = nix_config.flake_dir.clone();
    let flake_handle = tokio::task::spawn_blocking(move || {
        let mut inputs = match flake::read_flake_inputs(&flake_dir) {
            Ok(i) => i,
            Err(_) => return Vec::new(),
        };
        flake::check_flake_updates(&mut inputs);
        inputs
    });

    let lookups = repology::lookup_versions(&names).await?;
    let flake_inputs = flake_handle.await.unwrap_or_default();

    // Stop spinner
    done.store(true, Ordering::Relaxed);
    let _ = spinner.join();

    println!();
    let lookup_map: HashMap<String, repology::PackageLookup> = lookups
        .into_iter()
        .map(|l| (l.name.clone(), l))
        .collect();

    // 6. Compare versions and build results
    let mut minor_updates = Vec::new();
    let mut major_updates = Vec::new();
    let mut newer_results: Vec<CheckResult> = Vec::new();
    let mut unknown_names: Vec<String> = Vec::new();
    let mut up_to_date = 0;

    for (name, installed_version) in &packages_to_check {
        let lookup = match lookup_map.get(name) {
            Some(l) => l,
            None => {
                unknown_names.push(name.clone());
                continue;
            }
        };

        let available = match &lookup.version {
            Some(v) => v,
            None => {
                unknown_names.push(name.clone());
                continue;
            }
        };

        let installed_parts = parse_version(installed_version);
        let available_parts = parse_version(available);
        let diff = compare_versions(&installed_parts, &available_parts);

        let declared_in = names_with_files
            .get(name)
            .and_then(|files| files.first())
            .and_then(|p| {
                p.strip_prefix(&nix_config.flake_dir)
                    .ok()
                    .map(|r| r.display().to_string())
            });

        let result = CheckResult {
            name: name.clone(),
            installed: installed_version.clone(),
            available: available.clone(),
            declared_in,
        };

        match diff {
            VersionDiff::Equal => up_to_date += 1,
            VersionDiff::Minor => minor_updates.push(result),
            VersionDiff::Major => major_updates.push(result),
            VersionDiff::Newer => newer_results.push(result),
        }
    }
    let newer = newer_results.len();
    let unknown = unknown_names.len();

    // 7. Display results — flake inputs FIRST (most actionable), then packages.
    // Skip inputs that aren't referenced anywhere in the active modules
    // (typical case: a `zen-browser` input declared but never wired up).
    let used_inputs: std::collections::HashSet<String> = if category.is_none() {
        find_used_flake_inputs(&nix_config.flake_dir, active_set.as_ref())
    } else {
        flake_inputs.iter().map(|i| i.name.clone()).collect()
    };
    let visible_flake_inputs: Vec<&flake::FlakeInput> = flake_inputs
        .iter()
        .filter(|i| used_inputs.contains(&i.name))
        .collect();

    if category.is_none() && !visible_flake_inputs.is_empty() {
        let has_flake_updates = visible_flake_inputs.iter().any(|i| i.has_update == Some(true));
        if has_flake_updates {
            println!("{}:", "Flake inputs (updates available)".yellow().bold());
        } else {
            println!("{}:", "Flake inputs".bold());
        }

        for input in &visible_flake_inputs {
            let version_str = input.installed_version
                .as_deref()
                .unwrap_or("?");

            let status = match (&input.has_update, &input.remote_age) {
                (Some(true), Some(date)) => {
                    format!("{} ({})", "UPDATE".yellow(), date.dimmed())
                }
                (Some(true), None) => "UPDATE".yellow().to_string(),
                (Some(false), _) => "ok".green().to_string(),
                (None, _) => "?".dimmed().to_string(),
            };

            println!(
                "  {:<24} {:<14} {}",
                input.name,
                version_str.dimmed(),
                status,
            );
        }
        println!();
    }

    if !minor_updates.is_empty() {
        println!("{}:", "Updates available".yellow().bold());
        for r in &minor_updates {
            println!(
                "  {:<24} {:<14} {} {:<14} {} {}",
                r.name,
                r.installed.dimmed(),
                "→".dimmed(),
                r.available.green(),
                "(minor)".dimmed(),
                format_origin(r).dimmed(),
            );
        }
        println!();
    }

    if !major_updates.is_empty() {
        println!(
            "{} {}:",
            "Major updates".red().bold(),
            "(use 'cheni pin --force' to apply)".dimmed(),
        );
        for r in &major_updates {
            println!(
                "  {:<24} {:<14} {} {:<14} {} {}",
                r.name,
                r.installed.dimmed(),
                "→".dimmed(),
                r.available.red(),
                "(major)".red(),
                format_origin(r).dimmed(),
            );
        }
        println!();
    }

    if minor_updates.is_empty() && major_updates.is_empty() {
        println!("{}", "Everything is up to date!".green().bold());
        println!();
    }

    // --details: list packages in the Newer/Unknown buckets so the user
    // can spot mis-mappings, calver/semver clashes, or genuine pinned
    // ahead-of-nixpkgs cases.
    if details && !newer_results.is_empty() {
        println!(
            "{} {}",
            "Newer than nixpkgs".cyan().bold(),
            "(installed > available — usually fine, often a pinned package):".dimmed()
        );
        for r in &newer_results {
            println!(
                "  {:<24} {:<14} {} {:<14} {}",
                r.name,
                r.installed.cyan(),
                ">".dimmed(),
                r.available.dimmed(),
                format_origin(r).dimmed(),
            );
        }
        println!();
    }

    if details && !unknown_names.is_empty() {
        println!(
            "{} {}",
            "Unknown to Repology".dimmed().bold(),
            "(no version data — may need a name mapping):".dimmed()
        );
        for name in &unknown_names {
            println!("  {}", name);
        }
        println!();
    }

    if !details && (newer + unknown) > 0 {
        println!(
            "{}",
            "Tip: pass --details to list 'Newer' and 'Unknown' packages.".dimmed()
        );
    }

    // Summary line
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

/// Format the "in:" origin column for a check result. Returns either
/// "in modules/dev/foo.nix" or an empty string when we couldn't trace it.
fn format_origin(r: &CheckResult) -> String {
    match &r.declared_in {
        Some(path) => format!("in {}", path),
        None => String::new(),
    }
}

/// Return the set of flake inputs that are actually referenced from
/// the user's config. An input shows up here if any active `.nix` file
/// (or `flake.nix` itself, beyond the `inputs = { ... }` block) mentions
/// `inputs.<name>` or `inputs.<name>.something`.
///
/// Falls back to listing every input when active_set is None, so
/// non-NixOS-flake users still see their inputs in `cheni check`.
fn find_used_flake_inputs(
    flake_dir: &std::path::Path,
    active_set: Option<&std::collections::HashSet<std::path::PathBuf>>,
) -> std::collections::HashSet<String> {
    use std::collections::HashSet;

    let lock_text = std::fs::read_to_string(flake_dir.join("flake.lock")).unwrap_or_default();
    let lock: serde_json::Value = match serde_json::from_str(&lock_text) {
        Ok(v) => v,
        Err(_) => return HashSet::new(),
    };
    let all_inputs: Vec<String> = lock
        .get("nodes")
        .and_then(|n| n.get("root"))
        .and_then(|r| r.get("inputs"))
        .and_then(|i| i.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    let active = match active_set {
        Some(a) => a,
        None => return all_inputs.into_iter().collect(),
    };

    let mut used: HashSet<String> = HashSet::new();

    // Read every active .nix file once and grep textually for `inputs.<name>`.
    let mut texts: Vec<String> = active
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();

    // Also consider flake.nix itself — but only the part *outside* the
    // `inputs = { ... };` declaration block (every input is named there
    // and would otherwise look "used").
    let flake_text = std::fs::read_to_string(flake_dir.join("flake.nix")).unwrap_or_default();
    texts.push(strip_inputs_block(&flake_text));

    for name in all_inputs {
        let needle = format!("inputs.{}", name);
        if texts.iter().any(|t| t.contains(&needle)) {
            used.insert(name);
        }
    }
    used
}

/// Remove the top-level `inputs = { ... };` declaration from flake.nix
/// so that grepping for `inputs.<name>` inside the rest of the file
/// detects real usage instead of declarations.
fn strip_inputs_block(text: &str) -> String {
    let lower = text;
    let start = match lower.find("inputs") {
        Some(s) => s,
        None => return text.to_string(),
    };
    // Look for the opening brace after "inputs ="
    let after = &lower[start..];
    let eq = match after.find('=') {
        Some(e) => start + e,
        None => return text.to_string(),
    };
    let brace = match lower[eq..].find('{') {
        Some(b) => eq + b,
        None => return text.to_string(),
    };
    // Find matching closing brace (track depth).
    let bytes = lower.as_bytes();
    let mut depth = 0;
    let mut end = brace;
    for (i, &b) in bytes.iter().enumerate().skip(brace) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    out
}

/// Friendly explanation shown when `cheni init` has never been run.
/// Centralised here and reused by other gateway commands (pin, update).
pub fn print_first_run_hint() {
    println!("{}\n", "=== cheni — first run ===".bold());
    println!(
        "  Your flake doesn't declare {} yet, so per-package",
        "nixpkgs-latest".bold()
    );
    println!("  updates aren't available.");
    println!();
    println!("  Run '{}' to add the input + overlay automatically.", "cheni init".bold());
    println!();
    println!("  After that:");
    println!("    {} {}    {}", "•".cyan(), "cheni check".bold(), "see what's outdated".dimmed());
    println!("    {} {}      {}", "•".cyan(), "cheni pin <pkg>".bold(), "pin one package to update".dimmed());
    println!("    {} {}        {}", "•".cyan(), "cheni update".bold(), "apply pinned updates".dimmed());
    println!();
}
