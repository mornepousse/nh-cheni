//! `cheni check` command.
//!
//! Shows available updates for installed packages.
//! Compares local versions (from the nix store) with the latest
//! versions available on nixos-unstable (via Repology API).

use std::collections::HashMap;

use anyhow::Result;
use colored::Colorize;
use tracing::debug;

use serde::Serialize;

use crate::api::repology;
use crate::nix::{config, flake, pins, store};
use crate::version::compare::{compare_versions, VersionDiff};
use crate::version::parse::{is_prerelease, parse_version};

use super::obsolete::count_obsolete_pins;

/// A package with its update status, ready for display.
#[derive(Serialize)]
struct CheckResult {
    name: String,
    installed: String,
    available: String,
    /// Short relative path to the .nix file that declares this package
    /// (e.g. "modules/dev/esp-idf.nix"). None if we couldn't trace it back.
    declared_in: Option<String>,
}

/// JSON output schema — stable across versions so scripts can rely on it.
#[derive(Serialize)]
struct JsonOutput<'a> {
    flake_inputs: Vec<JsonFlakeInput<'a>>,
    minor_updates: &'a [CheckResult],
    major_updates: &'a [CheckResult],
    newer: &'a [CheckResult],
    unknown: &'a [String],
    summary: JsonSummary,
}

#[derive(Serialize)]
struct JsonFlakeInput<'a> {
    name: &'a str,
    installed: Option<&'a str>,
    has_update: Option<bool>,
    latest_remote_date: Option<&'a str>,
}

#[derive(Serialize)]
struct JsonSummary {
    up_to_date: usize,
    minor: usize,
    major: usize,
    newer: usize,
    unknown: usize,
}

/// Run the `cheni check` command.
///
/// If `category` is Some, only show packages from that module directory.
pub async fn run(category: Option<&str>, details: bool, json: bool, refresh: bool) -> Result<()> {
    if json {
        colored::control::set_override(false);
    }

    if refresh {
        // Nuke the on-disk cache so every lookup hits the API.
        // Useful after adjusting NAME_MAPPINGS, or when a package's
        // Repology entry just changed upstream.
        let _ = crate::api::cache::clear();
        if !json {
            println!("{}", "(cache cleared — re-fetching every lookup)".dimmed());
        }
    }

    // 1. Detect the NixOS configuration
    let nix_config = config::detect()?;

    if !config::is_initialized(&nix_config.flake_dir) {
        if json {
            println!("{}", serde_json::json!({"error": "not initialized"}));
            return Ok(());
        }
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
    let nix_files = gather_nix_files(&nix_config, category)?;

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

    let names_with_files = build_active_names_map(&nix_files, active_set.as_ref());
    debug!(
        "Config declares {} package names (active filter applied: {})",
        names_with_files.len(),
        active_set.is_some()
    );

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
    // Pass installed version as a hint so the matcher can disambiguate
    // Repology projects with multiple nix entries (e.g. exo / xfce4-exo,
    // libsForQt5.breeze-icons / kdePackages.breeze-icons).
    let names_with_installed: Vec<(String, Option<String>)> = packages_to_check
        .iter()
        .map(|(n, v)| (n.clone(), Some(v.clone())))
        .collect();
    let names: Vec<String> = packages_to_check.iter().map(|(n, _)| n.clone()).collect();
    let header = match category {
        Some(cat) => format!("Checking {} packages (modules/{}/) + flake inputs", names.len(), cat),
        None => format!("Checking {} packages + flake inputs...", names.len()),
    };
    if !json {
        println!("{}", header.dimmed());
    }

    let spinner = start_spinner(!json);

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

    let lookups = repology::lookup_versions(&names_with_installed).await?;
    let flake_inputs = flake_handle.await.unwrap_or_default();

    spinner.stop();

    println!();
    let lookup_map: HashMap<String, repology::PackageLookup> = lookups
        .into_iter()
        .map(|l| (l.name.clone(), l))
        .collect();

    // 6. Compare versions and build results
    let classification = classify_lookups(
        &packages_to_check,
        &lookup_map,
        &names_with_files,
        &nix_config.flake_dir,
    );

    // 7. Filter flake inputs to the ones actually referenced. Avoids
    // listing a declared-but-unused `zen-browser` input as if the user
    // cared about its updates.
    let used_inputs: std::collections::HashSet<String> = if category.is_none() {
        find_used_flake_inputs(&nix_config.flake_dir, active_set.as_ref())
    } else {
        flake_inputs.iter().map(|i| i.name.clone()).collect()
    };
    let visible_flake_inputs: Vec<&flake::FlakeInput> = flake_inputs
        .iter()
        .filter(|i| used_inputs.contains(&i.name))
        .collect();

    if json {
        print_json(&classification, &visible_flake_inputs)?;
    } else {
        print_human(&classification, &visible_flake_inputs, category, details);
    }
    Ok(())
}

/// Bucketed result of running `compare_versions` over every queried
/// package. Used as the single carry between the scanning phase and
/// the rendering phase.
struct Classification {
    minor: Vec<CheckResult>,
    major: Vec<CheckResult>,
    newer: Vec<CheckResult>,
    unknown: Vec<String>,
    up_to_date: usize,
}

/// Compare each (name, installed_version) tuple against its Repology
/// lookup and bucket the outcome. Pre-release "available" versions are
/// treated as up-to-date when the installed version is stable —
/// otherwise Repology's latest (e.g. python 3.15.0a7) would
/// permanently surface as a minor update against 3.14.3.
fn classify_lookups(
    packages_to_check: &[(String, String)],
    lookup_map: &HashMap<String, repology::PackageLookup>,
    names_with_files: &HashMap<String, Vec<std::path::PathBuf>>,
    flake_dir: &std::path::Path,
) -> Classification {
    let mut c = Classification {
        minor: Vec::new(),
        major: Vec::new(),
        newer: Vec::new(),
        unknown: Vec::new(),
        up_to_date: 0,
    };

    for (name, installed_version) in packages_to_check {
        let Some(lookup) = lookup_map.get(name) else {
            c.unknown.push(name.clone());
            continue;
        };
        let Some(available) = &lookup.version else {
            c.unknown.push(name.clone());
            continue;
        };
        if is_prerelease(available) && !is_prerelease(installed_version) {
            c.up_to_date += 1;
            continue;
        }

        let diff = compare_versions(
            &parse_version(installed_version),
            &parse_version(available),
        );
        let declared_in = names_with_files
            .get(name)
            .and_then(|files| files.first())
            .and_then(|p| {
                p.strip_prefix(flake_dir)
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
            VersionDiff::Equal => c.up_to_date += 1,
            VersionDiff::Minor => c.minor.push(result),
            VersionDiff::Major => c.major.push(result),
            VersionDiff::Newer => c.newer.push(result),
        }
    }
    c
}

/// Render the classification as the stable JSON document for scripts.
fn print_json(
    c: &Classification,
    visible_flake_inputs: &[&flake::FlakeInput],
) -> Result<()> {
    let out = JsonOutput {
        flake_inputs: visible_flake_inputs
            .iter()
            .map(|i| JsonFlakeInput {
                name: &i.name,
                installed: i.installed_version.as_deref(),
                has_update: i.has_update,
                latest_remote_date: i.remote_age.as_deref(),
            })
            .collect(),
        minor_updates: &c.minor,
        major_updates: &c.major,
        newer: &c.newer,
        unknown: &c.unknown,
        summary: JsonSummary {
            up_to_date: c.up_to_date,
            minor: c.minor.len(),
            major: c.major.len(),
            newer: c.newer.len(),
            unknown: c.unknown.len(),
        },
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Render the classification as the colourful human report.
fn print_human(
    c: &Classification,
    visible_flake_inputs: &[&flake::FlakeInput],
    category: Option<&str>,
    details: bool,
) {
    if category.is_none() && !visible_flake_inputs.is_empty() {
        print_flake_inputs_block(visible_flake_inputs);
    }
    if !c.minor.is_empty() {
        print_update_block("Updates available", &c.minor, "minor", true);
    }
    if !c.major.is_empty() {
        print_update_block(
            "Major updates",
            &c.major,
            "major",
            false,
        );
    }
    if c.minor.is_empty() && c.major.is_empty() {
        println!("{}\n", "Everything is up to date!".green().bold());
    }
    if details && !c.newer.is_empty() {
        print_newer_block(&c.newer);
    }
    if details && !c.unknown.is_empty() {
        print_unknown_block(&c.unknown);
    }
    if !details && (c.newer.len() + c.unknown.len()) > 0 {
        println!(
            "{}",
            "Tip: pass --details to list 'Newer' and 'Unknown' packages.".dimmed()
        );
    }
    println!(
        "{} {} | {} {} | {} {} | {} {} | {} {}",
        "Up to date:".dimmed(),
        c.up_to_date.to_string().green(),
        "Minor:".dimmed(),
        c.minor.len().to_string().yellow(),
        "Major:".dimmed(),
        c.major.len().to_string().red(),
        "Newer:".dimmed(),
        c.newer.len().to_string().cyan(),
        "Unknown:".dimmed(),
        c.unknown.len().to_string().dimmed(),
    );
}

fn print_flake_inputs_block(inputs: &[&flake::FlakeInput]) {
    let has_updates = inputs.iter().any(|i| i.has_update == Some(true));
    let header = if has_updates {
        "Flake inputs (updates available)".yellow().bold()
    } else {
        "Flake inputs".bold()
    };
    println!("{}:", header);
    for input in inputs {
        let version = input.installed_version.as_deref().unwrap_or("?");
        let status = match (&input.has_update, &input.remote_age) {
            (Some(true), Some(date)) => format!("{} ({})", "UPDATE".yellow(), date.dimmed()),
            (Some(true), None) => "UPDATE".yellow().to_string(),
            (Some(false), _) => "ok".green().to_string(),
            (None, _) => "?".dimmed().to_string(),
        };
        println!("  {:<24} {:<14} {}", input.name, version.dimmed(), status);
    }
    println!();
}

fn print_update_block(header: &str, updates: &[CheckResult], tag: &str, minor: bool) {
    if minor {
        println!("{}:", header.yellow().bold());
    } else {
        println!(
            "{} {}:",
            header.red().bold(),
            "(use 'cheni pin --force' to apply)".dimmed()
        );
    }
    let tag_label = if minor {
        format!("({})", tag).dimmed()
    } else {
        format!("({})", tag).red()
    };
    for r in updates {
        let new_ver = if minor {
            r.available.green()
        } else {
            r.available.red()
        };
        println!(
            "  {:<24} {:<14} {} {:<14} {} {}",
            r.name,
            r.installed.dimmed(),
            "→".dimmed(),
            new_ver,
            tag_label,
            format_origin(r).dimmed(),
        );
    }
    println!();
}

fn print_newer_block(newer: &[CheckResult]) {
    println!(
        "{} {}",
        "Newer than nixpkgs".cyan().bold(),
        "(installed > available — usually fine, often a pinned package):".dimmed()
    );
    for r in newer {
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

fn print_unknown_block(unknown: &[String]) {
    println!(
        "{} {}",
        "Unknown to Repology".dimmed().bold(),
        "(no version data — may need a name mapping):".dimmed()
    );
    for name in unknown {
        println!("  {}", name);
    }
    println!();
}

/// Collect every `.nix` file that the check should scan for package
/// declarations. Either a single category (`-c dev` → modules/dev/) or
/// the union of all module categories plus `home/`.
fn gather_nix_files(
    nix_config: &config::NixConfig,
    category: Option<&str>,
) -> Result<Vec<std::path::PathBuf>> {
    if let Some(cat) = category {
        let files = config::list_module_files(&nix_config.flake_dir, cat);
        if files.is_empty() {
            anyhow::bail!(
                "No module category '{}' found.\nAvailable: {}",
                cat,
                config::list_module_categories(&nix_config.flake_dir).join(", ")
            );
        }
        return Ok(files);
    }

    let mut files = Vec::new();
    for cat in config::list_module_categories(&nix_config.flake_dir) {
        files.extend(config::list_module_files(&nix_config.flake_dir, &cat));
    }
    let home_dir = nix_config.flake_dir.join("home");
    if home_dir.exists() {
        let base_dir = home_dir.parent().unwrap_or(&nix_config.flake_dir);
        files.extend(config::list_module_files(base_dir, "home"));
    }
    Ok(files)
}

/// Build the package-name → declaring-files map, optionally restricted
/// to files that are actually imported by the active host config.
///
/// `canonicalize` is called once per unique path (not per package),
/// because the same .nix file is typically referenced by dozens of
/// packages.
fn build_active_names_map(
    nix_files: &[std::path::PathBuf],
    active_set: Option<&std::collections::HashSet<std::path::PathBuf>>,
) -> std::collections::HashMap<String, Vec<std::path::PathBuf>> {
    let mut names_with_files = config::extract_package_names_with_files(nix_files);
    let Some(active) = active_set else {
        return names_with_files;
    };
    let mut canon_cache: std::collections::HashMap<std::path::PathBuf, bool> =
        std::collections::HashMap::new();
    for paths in names_with_files.values_mut() {
        paths.retain(|p| {
            *canon_cache.entry(p.clone()).or_insert_with(|| {
                p.canonicalize()
                    .ok()
                    .map(|c| active.contains(&c))
                    .unwrap_or(false)
            })
        });
    }
    names_with_files.retain(|_, paths| !paths.is_empty());
    names_with_files
}

/// Background spinner shown on stderr while remote APIs are queried.
/// Auto-disabled on non-TTY stderr or when `enabled` is false (e.g.
/// `--json` mode), so it never pollutes piped output.
struct Spinner {
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

impl Spinner {
    fn stop(self) {
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.handle.join();
    }
}

fn start_spinner(enabled: bool) -> Spinner {
    use std::io::IsTerminal;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let done = Arc::new(AtomicBool::new(false));
    let done_clone = done.clone();
    let active = enabled && std::io::stderr().is_terminal();
    let handle = std::thread::spawn(move || {
        if !active {
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
    Spinner { done, handle }
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
