//! `cheni check` command.
//!
//! Shows available updates for installed packages.
//! Compares local versions (from the nix store) with the latest
//! versions available in `nixpkgs-latest` (via nix eval).

use std::collections::HashMap;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use serde::Serialize;

use crate::nix::{config, flake, freezes, pins, store};
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
    frozen: Vec<JsonFrozen<'a>>,
    summary: JsonSummary,
}

#[derive(Serialize)]
struct JsonFrozen<'a> {
    name: &'a str,
    version: &'a str,
    frozen_at: &'a str,
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
    frozen: usize,
}

/// Run the `cheni check` command.
///
/// If `category` is Some, only show packages from that module directory.
pub async fn run(
    category: Option<&str>,
    details: bool,
    json: bool,
    refresh: bool,
    pending: bool,
) -> Result<()> {
    apply_early_flags(json, refresh);

    let nix_config = config::detect()?;
    if !handle_init_check(&nix_config.flake_dir, json) {
        return Ok(());
    }
    warn_about_obsolete_pins(&nix_config.flake_dir)?;

    let Some(mut scan) = gather_packages_to_check(&nix_config, category)? else {
        println!("{}", "No packages found to check.".dimmed());
        return Ok(());
    };

    // Exclude frozen packages from the Repology check: the user has
    // deliberately held them at a past version, so there's no update
    // decision to make. We still surface them in a "Frozen" block so
    // they're not invisible in the report.
    let current_freezes = freezes::read(&nix_config.flake_dir)?;
    let frozen_rows = split_out_frozen(&mut scan.packages, &current_freezes);

    print_check_header(&scan, category, json);
    let (lookup_map, flake_inputs) = fetch_updates_concurrently(&nix_config, &scan, json).await?;

    let classification = classify_lookups(
        &scan.packages,
        &lookup_map,
        &scan.names_with_files,
        &nix_config.flake_dir,
    );

    let visible_flake_inputs =
        filter_visible_flake_inputs(&flake_inputs, &nix_config.flake_dir, scan.active_set.as_ref(), category);

    // Read `nixpkgs` straight from the lock — `read_flake_inputs`
    // excludes it by design (no per-pin update suggestion), but the
    // freshness signal still has to surface so the user can tell
    // "no updates available" from "behind reality by 12 days".
    let nixpkgs_age = flake::read_input_by_name(&nix_config.flake_dir, "nixpkgs");

    if json {
        print_json(&classification, &visible_flake_inputs, &frozen_rows)?;
    } else {
        print_human(
            &classification,
            &visible_flake_inputs,
            &frozen_rows,
            category,
            details,
            nixpkgs_age.as_ref(),
        );
    }

    // Optional second pass: closure-level dry-run, surfaces what
    // would actually rebuild — kernel, base system, transitive deps.
    // Distinct view from the nix-eval section above (which only
    // sees module-named packages present in nixpkgs). Skipped
    // by default because it adds 30–60s of evaluation; --json
    // ignores it because the section isn't represented in the
    // schema yet (machine consumers usually want the structured
    // nix-eval view alone).
    if pending && !json {
        append_pending_section(&nix_config)?;
    }

    // Best-effort self-update hint at the very tail. Cached for 24h
    // so the GitLab API isn't hit on every check. Silent on failure
    // — `cheni check` must keep working offline / under rate limit.
    if !json {
        maybe_print_self_update_hint(&nix_config.flake_dir).await;
    }
    Ok(())
}

/// When the user pinned cheni at a release tag and a newer tag is
/// available on GitLab, print a one-line invitation to run
/// `cheni self-update`. Skipped silently for branch-tracking pins
/// (those bump on plain `nix flake update`), unparseable lock
/// files, and any GitLab API trouble — this is decoration, not a
/// gate.
async fn maybe_print_self_update_hint(flake_dir: &std::path::Path) {
    let current_tag = match super::self_update::read_cheni_tag(flake_dir) {
        Ok(t) => t,
        Err(e) => {
            debug!("self-update hint skipped: {}", e);
            return;
        }
    };
    if !crate::release::is_release_tag(&current_tag) {
        return;
    }
    let latest = match crate::release::latest_release_tag_cached().await {
        Ok(t) => t,
        Err(e) => {
            debug!("self-update hint skipped: {}", e);
            return;
        }
    };
    if latest == current_tag {
        return;
    }
    // Anti-downgrade: a transient API quirk shouldn't make us
    // recommend going backwards.
    let cur_v = crate::version::parse::parse_version(
        current_tag.strip_prefix('v').unwrap_or(&current_tag),
    );
    let lat_v =
        crate::version::parse::parse_version(latest.strip_prefix('v').unwrap_or(&latest));
    if lat_v <= cur_v {
        return;
    }
    println!();
    println!(
        "  {} cheni {} available (you're on {}) — run '{}' to update.",
        "→".cyan(),
        latest.bold(),
        current_tag.dimmed(),
        "cheni self-update".bold()
    );
}

/// Print the "Pending closure changes" section under a regular
/// `cheni check` run. Re-uses the dry-run + render pipeline from
/// `cheni upgrade` step 2 so the format stays in sync, and prefixes
/// the `flake.lock` dirty warning when relevant — same caveats as
/// in upgrade, since both are reading the same lock file state.
fn append_pending_section(nix_config: &config::NixConfig) -> Result<()> {
    println!();
    println!(
        "{}",
        "─── Pending closure changes (dry-run) ──────────────".bold()
    );
    println!(
        "{}",
        "What would change at the next `cheni upgrade` or `cheni build` —"
            .dimmed()
    );
    println!(
        "{}",
        "kernel, base nixpkgs packages, transitive deps included.".dimmed()
    );
    println!();

    super::upgrade::warn_if_dirty_lock(&nix_config.flake_dir);
    let config_path = nix_config
        .flake_dir
        .to_str()
        .context("Config path is not valid UTF-8")?;
    super::upgrade::print_pending_changes(config_path, &nix_config.hostname)?;
    Ok(())
}

/// Frozen-package row for display. Mirrors `CheckResult` at a higher
/// level — we don't do Repology lookups for frozen entries, so the
/// report comes straight from `package-freezes.json` + the store.
struct FrozenRow {
    name: String,
    version: String,
    frozen_at: String,
}

/// Remove frozen packages from the to-check list in place, returning
/// a `FrozenRow` for each one so the report can still surface them.
/// Operates on the vector rather than rebuilding it to preserve the
/// original ordering of the non-frozen packages.
fn split_out_frozen(
    packages: &mut Vec<(String, String)>,
    current_freezes: &freezes::Freezes,
) -> Vec<FrozenRow> {
    if current_freezes.is_empty() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    packages.retain(|(name, version)| match current_freezes.get(name) {
        Some(entry) => {
            rows.push(FrozenRow {
                name: name.clone(),
                version: version.clone(),
                frozen_at: entry.frozen_at.clone(),
            });
            false
        }
        None => true,
    });
    rows
}

/// Apply the two flags that affect the rest of the run before any
/// work is done: `--json` disables colour, `--refresh` wipes the
/// version cache so every eval re-runs.
fn apply_early_flags(json: bool, refresh: bool) {
    if json {
        colored::control::set_override(false);
    }
    if refresh {
        // Useful after updating flake inputs or when a package's
        // nixpkgs-latest entry changed.
        let _ = crate::nix::version_cache::clear();
        if !json {
            println!("{}", "(cache cleared — re-evaluating every lookup)".dimmed());
        }
    }
}

/// Returns true when the flake is initialised and the caller should
/// proceed. Returns false (after printing the right message) otherwise.
/// JSON mode gets a machine-readable `{"error": "not initialized"}`
/// instead of the prose hint.
fn handle_init_check(flake_dir: &std::path::Path, json: bool) -> bool {
    if config::is_initialized(flake_dir) {
        return true;
    }
    if json {
        println!("{}", serde_json::json!({"error": "not initialized"}));
    } else {
        print_first_run_hint();
    }
    false
}

/// Prints a single-line warning when there are obsolete pins in
/// `package-pins.json` — packages pinned to nixpkgs-latest for which
/// nixpkgs itself has since caught up, so the pin is now a no-op.
fn warn_about_obsolete_pins(flake_dir: &std::path::Path) -> Result<()> {
    let current_pins = pins::read(flake_dir)?;
    if current_pins.is_empty() {
        return Ok(());
    }
    let obsolete = count_obsolete_pins(&flake_dir.join("flake.lock"), &current_pins);
    if obsolete > 0 {
        println!(
            "{} {} obsolete {} detected. Run '{}' to remove.\n",
            "Note:".yellow(),
            obsolete,
            crate::util::pluralize(obsolete, "pin"),
            "cheni clean".bold()
        );
    }
    Ok(())
}

/// Everything we need to drive the API query phase: the (name, version)
/// pairs to check, the reverse map from name to declaring file(s), and
/// the "active modules" filter that scopes the `in <file>` attribution.
struct PackagesToCheck {
    /// (name, installed version) pairs intersected with the store.
    packages: Vec<(String, String)>,
    /// name → declaring file(s), for the "in modules/.../foo.nix" hint.
    names_with_files: std::collections::HashMap<String, Vec<std::path::PathBuf>>,
    /// Set of `.nix` files actively imported by the host config, or
    /// None when the user asked for a category (show everything).
    active_set: Option<std::collections::HashSet<std::path::PathBuf>>,
}

/// Cross-reference the nix store with package references in the user's
/// `.nix` config. Returns None when the intersection is empty (i.e.
/// there's nothing worth querying the API about).
fn gather_packages_to_check(
    nix_config: &config::NixConfig,
    category: Option<&str>,
) -> Result<Option<PackagesToCheck>> {
    let store_packages = store::read_installed_packages()?;
    let store_map: HashMap<String, String> = store_packages
        .iter()
        .map(|p| (p.name.to_lowercase(), p.version.clone()))
        .collect();

    let nix_files = gather_nix_files(nix_config, category)?;

    // When the user explicitly picks a category, show everything inside
    // it. Otherwise restrict attribution to files imported by the host
    // config — commented-out or orphan modules would otherwise be
    // reported as the source of the package reference.
    let active_set: Option<std::collections::HashSet<std::path::PathBuf>> = if category.is_some() {
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

    let mut packages: Vec<(String, String)> = Vec::new();
    let mut sorted_names: Vec<&String> = names_with_files.keys().collect();
    sorted_names.sort();
    for name in &sorted_names {
        if let Some(version) = store_map.get(&name.to_lowercase()) {
            packages.push(((*name).clone(), version.clone()));
        }
    }

    if packages.is_empty() {
        return Ok(None);
    }
    Ok(Some(PackagesToCheck { packages, names_with_files, active_set }))
}

fn print_check_header(scan: &PackagesToCheck, category: Option<&str>, json: bool) {
    if json {
        return;
    }
    let header = match category {
        Some(cat) => format!(
            "Checking {} packages (modules/{}/) + flake inputs",
            scan.packages.len(),
            cat
        ),
        None => format!("Checking {} packages + flake inputs...", scan.packages.len()),
    };
    println!("{}", header.dimmed());
}

/// Run the nix-eval lookups and the flake-input update probes
/// concurrently — the eval loop runs in `spawn_blocking` (nix is a
/// sync subprocess), flake checks fan out on the same pool. On return
/// the spinner is stopped and a blank line printed so subsequent output
/// starts cleanly.
async fn fetch_updates_concurrently(
    nix_config: &config::NixConfig,
    scan: &PackagesToCheck,
    json: bool,
) -> Result<(HashMap<String, Option<String>>, Vec<flake::FlakeInput>)> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let total_packages = scan.packages.len();
    let resolved = Arc::new(AtomicUsize::new(0));
    let flake_done = Arc::new(AtomicBool::new(false));

    let spinner = start_spinner(
        !json,
        resolved.clone(),
        total_packages,
        flake_done.clone(),
    );

    let flake_dir = nix_config.flake_dir.clone();
    let flake_done_for_task = flake_done.clone();
    let flake_handle = tokio::task::spawn_blocking(move || {
        let Ok(mut inputs) = flake::read_flake_inputs(&flake_dir) else {
            flake_done_for_task.store(true, Ordering::Relaxed);
            return Vec::new();
        };
        flake::check_flake_updates(&mut inputs);
        flake_done_for_task.store(true, Ordering::Relaxed);
        inputs
    });

    // Eval loop in spawn_blocking — `nix eval` is a sync subprocess and
    // blocking the tokio runtime directly would stall all async tasks.
    let flake_dir_for_eval = nix_config.flake_dir.clone();
    let names: Vec<String> = scan.packages.iter().map(|(n, _)| n.clone()).collect();
    let resolved_for_eval = resolved.clone();
    let eval_handle = tokio::task::spawn_blocking(
        move || -> Result<HashMap<String, Option<String>>> {
            use crate::nix::eval::lookup_or_eval;
            use crate::nix::flake::{read_input_rev, target_system};
            use crate::nix::version_cache::{cache_path, VersionCache};

            let cache_path = cache_path();
            let mut cache = VersionCache::load(&cache_path).unwrap_or_default();
            let rev = read_input_rev(&flake_dir_for_eval, "nixpkgs-latest");
            let system = target_system();

            let mut out: HashMap<String, Option<String>> = HashMap::with_capacity(names.len());
            for name in names {
                let result = match &rev {
                    Some(rev) => {
                        let attr = format!("legacyPackages.{system}.{name}");
                        lookup_or_eval(&mut cache, "nixpkgs-latest", rev, &attr)
                            .ok()
                            .flatten()
                    }
                    None => None,
                };
                out.insert(name, result);
                resolved_for_eval.fetch_add(1, Ordering::Relaxed);
            }

            // Best-effort save — if it fails (disk full, perms) the run still works.
            let _ = cache.save(&cache_path);
            Ok(out)
        },
    );

    let lookups = eval_handle.await.unwrap_or_else(|_| Ok(HashMap::new()))?;
    let flake_inputs = flake_handle.await.unwrap_or_default();
    spinner.stop();
    println!();
    Ok((lookups, flake_inputs))
}

/// Filter flake inputs to the ones actually referenced by the active
/// host configuration. Avoids listing a declared-but-unused
/// `zen-browser` input as if the user cared about its updates.
/// In category mode, everything is shown (the user asked for it).
fn filter_visible_flake_inputs<'a>(
    flake_inputs: &'a [flake::FlakeInput],
    flake_dir: &std::path::Path,
    active_set: Option<&std::collections::HashSet<std::path::PathBuf>>,
    category: Option<&str>,
) -> Vec<&'a flake::FlakeInput> {
    let used: std::collections::HashSet<String> = if category.is_none() {
        find_used_flake_inputs(flake_dir, active_set)
    } else {
        flake_inputs.iter().map(|i| i.name.clone()).collect()
    };
    flake_inputs.iter().filter(|i| used.contains(&i.name)).collect()
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

/// Compare each (name, installed_version) tuple against its nix-eval
/// lookup and bucket the outcome. Pre-release "available" versions are
/// treated as up-to-date when the installed version is stable —
/// otherwise nixpkgs-latest (e.g. python 3.15.0a7) would
/// permanently surface as a minor update against 3.14.3.
fn classify_lookups(
    packages_to_check: &[(String, String)],
    lookup_map: &HashMap<String, Option<String>>,
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
        let Some(available_opt) = lookup_map.get(name) else {
            c.unknown.push(name.clone());
            continue;
        };
        let Some(available) = available_opt else {
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
    frozen_rows: &[FrozenRow],
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
        frozen: frozen_rows
            .iter()
            .map(|r| JsonFrozen {
                name: &r.name,
                version: &r.version,
                frozen_at: &r.frozen_at,
            })
            .collect(),
        summary: JsonSummary {
            up_to_date: c.up_to_date,
            minor: c.minor.len(),
            major: c.major.len(),
            newer: c.newer.len(),
            unknown: c.unknown.len(),
            frozen: frozen_rows.len(),
        },
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Render the classification as the colourful human report.
fn print_human(
    c: &Classification,
    visible_flake_inputs: &[&flake::FlakeInput],
    frozen_rows: &[FrozenRow],
    category: Option<&str>,
    details: bool,
    nixpkgs: Option<&flake::FlakeInput>,
) {
    if category.is_none() {
        print_nixpkgs_age_block(nixpkgs);
        if !visible_flake_inputs.is_empty() {
            print_flake_inputs_block(visible_flake_inputs);
        }
    }
    if !frozen_rows.is_empty() {
        print_frozen_block(frozen_rows);
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
    let frozen_tail = if frozen_rows.is_empty() {
        String::new()
    } else {
        format!(" | {} {}",
            "Frozen:".dimmed(),
            frozen_rows.len().to_string().cyan())
    };
    println!(
        "{} {} | {} {} | {} {} | {} {} | {} {}{}",
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
        frozen_tail,
    );
    if let Some(message) = suspicious_eval_silence(c) {
        println!();
        println!("  {} {}", "⚠".yellow().bold(), message.yellow());
    }
}

/// Detect the "all packages are unknown" signature: every classified
/// package landed in Unknown bucket. Now that the lookup is via nix
/// eval against `nixpkgs-latest`, this typically means:
///   - the user has no `nixpkgs-latest` input in their flake.lock
///   - or `nix eval` is systemically failing (broken store, network
///     for fetchTree, evaluator crash)
///
/// Stays silent for small configs (< 10 packages) where the outcome
/// can be legitimate. Returns `None` for normal reports.
fn suspicious_eval_silence(c: &Classification) -> Option<String> {
    let classified = c.up_to_date + c.minor.len() + c.major.len() + c.newer.len();
    if classified > 0 || c.unknown.len() < 10 {
        return None;
    }
    Some(format!(
        "All {} package lookups returned Unknown — likely missing \
         `nixpkgs-latest` input in flake.lock, or systemic nix eval \
         failure. Run with `-v` to see debug-level eval errors.",
        c.unknown.len()
    ))
}

/// Render the "Frozen" block. No Repology column — the user's intent
/// for these packages is "don't tell me about updates", so we just
/// reaffirm the held version and freeze date.
fn print_frozen_block(rows: &[FrozenRow]) {
    println!("{}:", "Frozen (held at their snapshot)".cyan().bold());
    for r in rows {
        println!(
            "  {:<24} {:<14} {}",
            r.name,
            r.version.dimmed(),
            format!("(since {})", r.frozen_at).dimmed()
        );
    }
    println!();
}

/// Render a one-line nixpkgs age header above the flake-inputs block.
/// This is the freshness baseline for everything `cheni check` says
/// next: a 14-day-old nixpkgs explains why a "0 updates" report
/// looks suspiciously quiet. Skipped silently when the input can't
/// be located in the lock.
fn print_nixpkgs_age_block(nixpkgs: Option<&flake::FlakeInput>) {
    let Some(input) = nixpkgs else {
        return;
    };
    let age = format_local_age(input.days_old);
    println!(
        "{} {} {}",
        "nixpkgs floor:".bold(),
        age.dimmed(),
        format!("(rev {})", input.rev).dimmed(),
    );
    if input.days_old >= 3 {
        println!(
            "  {} run `{}` to advance the floor before re-checking",
            "→".cyan(),
            "cheni upgrade".bold()
        );
    }
    println!();
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
        let local_age = format_local_age(input.days_old);
        let status = match (&input.has_update, &input.remote_age) {
            (Some(true), Some(date)) => format!("{} ({})", "UPDATE".yellow(), date.dimmed()),
            (Some(true), None) => "UPDATE".yellow().to_string(),
            (Some(false), _) => "ok".green().to_string(),
            (None, _) => "?".dimmed().to_string(),
        };
        println!(
            "  {:<24} {:<14} {:<13} {}",
            input.name,
            version.dimmed(),
            local_age.dimmed(),
            status,
        );
    }
    println!();
}

/// Local alias for the shared `util::format_days_ago` so the call
/// sites in `check` keep their narrow naming. All "Xd ago" surfaces
/// in cheni share the same helper now.
fn format_local_age(days: u64) -> String {
    crate::util::format_days_ago(days)
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
        "Unknown to nixpkgs-latest".dimmed().bold(),
        "(no version data — package may not exist in nixpkgs):".dimmed()
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

/// Start a live progress indicator reading from two signals:
/// - `resolved` — number of Repology packages resolved (cache hit or
///   API response). `total` is the denominator.
/// - `flake_done` — flipped to true once the flake-input probe finishes.
///
/// Renders one line that updates in place, so the output stays quiet
/// instead of scrolling.
fn start_spinner(
    enabled: bool,
    resolved: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    total: usize,
    flake_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Spinner {
    use std::io::{IsTerminal, Write};
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
        let mut last_line_len = 0;
        while !done_clone.load(Ordering::Relaxed) {
            let evaluated = resolved.load(Ordering::Relaxed);
            let flake = if flake_done.load(Ordering::Relaxed) { "✓" } else { "…" };
            let line = format!(
                "  {} packages {}/{}  ·  flake inputs {}",
                frames[i % frames.len()],
                evaluated.min(total),
                total,
                flake,
            );
            // Overwrite to the widest line seen so far so shrinking
            // text (e.g. count goes from "12" to "120" and back to "12"
            // across wrapped terminal edges) leaves no ghost characters.
            let pad = if line.len() < last_line_len {
                last_line_len - line.len()
            } else {
                0
            };
            eprint!("\r{}{}", line, " ".repeat(pad));
            let _ = std::io::stderr().flush();
            last_line_len = last_line_len.max(line.len());
            std::thread::sleep(std::time::Duration::from_millis(100));
            i += 1;
        }
        // Clear the indicator line so the report that follows starts at
        // column 0 without any leftover progress text.
        eprint!("\r{}\r", " ".repeat(last_line_len));
        let _ = std::io::stderr().flush();
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

#[cfg(test)]
#[path = "tests/check.rs"]
mod tests;

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
    println!(
        "    {} {}  {}",
        "•".cyan(),
        "cheni upgrade --pins-only".bold(),
        "apply pinned updates".dimmed()
    );
    println!();
}
