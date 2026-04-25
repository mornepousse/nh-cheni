//! `cheni search` command.
//!
//! Searches nixpkgs for packages matching a query, then enriches each
//! displayed hit with cross-context information that `nix search`
//! alone cannot surface:
//!
//! - **Repology delta** — when Repology's known upstream version
//!   differs from the nixpkgs version, the row gets a `→ <version>
//!   upstream` annotation. Helps decide "is this a stale package or
//!   the latest?".
//! - **Local state** — when the package is already in the user's
//!   modules, pinned, or frozen, the row gets the matching badge
//!   (`installed`, `pinned`, `frozen@<v>`).
//!
//! This is the primary cheni vs. nh differentiator: nh has no notion
//! of pins/freezes nor a Repology client.

use std::collections::HashSet;
use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use crate::api::repology;
use crate::nix::{config, freezes, pins};

/// One result row from `nix search`: (short attr name, version, description).
type SearchRow = (String, String, String);

/// Maximum number of search hits we render. Past this threshold, the
/// user is fishing — we'd be making API calls for results they won't
/// look at.
const MAX_DISPLAY: usize = 30;

/// Maximum number of hits we look up on Repology.
///
/// Capped well below `MAX_DISPLAY` because each lookup pays the
/// repology rate-limit dance (see `repology::MAX_CONCURRENT`). The
/// top-relevance hits are what users actually inspect; deeper hits
/// stay un-annotated and that's fine.
const MAX_REPOLOGY_LOOKUPS: usize = 10;

/// Run `cheni search <query>`.
///
/// Uses `nix search nixpkgs <query>` to find matching packages, then
/// concurrently asks Repology for the top hits' upstream versions and
/// looks up local pin/freeze/installed state. Both enrichments degrade
/// silently on failure — the base `nix search` output always renders.
pub async fn run(query: &str) -> Result<()> {
    println!("{} {}\n", "Searching nixpkgs for".dimmed(), query.bold());

    let raw = run_nix_search(query)?;
    let Some(obj) = raw.as_object() else {
        println!("{}", "No packages found.".dimmed());
        return Ok(());
    };
    if obj.is_empty() {
        println!("{}", "No packages found.".dimmed());
        return Ok(());
    }
    debug!("Found {} results", obj.len());

    let q = query.to_lowercase();
    let results = parse_and_sort_results(obj, &q);
    let displayed: Vec<SearchRow> = results.iter().take(MAX_DISPLAY).cloned().collect();

    let local = gather_local_state();
    let upstream = lookup_upstream(&displayed).await;

    print_results(&displayed, &q, &local, &upstream);
    print_footer(results.len());
    Ok(())
}

/// Local state of a single package — collected once and read by row.
///
/// The booleans + the freeze version are checked against the package's
/// short name (matching the column the user reads in the search list).
struct LocalState {
    installed: HashSet<String>,
    pinned: HashSet<String>,
    /// Map name → frozen version (empty string if the freeze entry has
    /// no recorded version, which happens for older freeze files).
    frozen: std::collections::HashMap<String, String>,
}

impl LocalState {
    fn empty() -> Self {
        LocalState {
            installed: HashSet::new(),
            pinned: HashSet::new(),
            frozen: std::collections::HashMap::new(),
        }
    }

    /// Render the local-state markers for `name` joined with ` · `,
    /// or `None` if the package isn't tracked locally at all.
    fn badges(&self, name: &str) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = self.frozen.get(name) {
            parts.push(if v.is_empty() {
                "frozen".to_string()
            } else {
                format!("frozen@{}", v)
            });
        }
        if self.pinned.contains(name) {
            parts.push("pinned".to_string());
        }
        if self.installed.contains(name) {
            parts.push("installed".to_string());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }
}

/// Resolve the user's flake config and load installed/pinned/frozen
/// names. Returns an empty state when no flake is found — search still
/// works, just without the "[pinned]" / "[installed]" badges.
fn gather_local_state() -> LocalState {
    let Ok(cfg) = config::detect() else {
        debug!("no flake detected — search badges skipped");
        return LocalState::empty();
    };
    let pinned: HashSet<String> = pins::read(&cfg.flake_dir)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let frozen: std::collections::HashMap<String, String> = freezes::read(&cfg.flake_dir)
        .unwrap_or_default()
        .into_iter()
        .map(|(name, entry)| (name, entry.version))
        .collect();
    let installed = collect_installed_names(&cfg);
    LocalState {
        installed,
        pinned,
        frozen,
    }
}

/// Collect every package name declared in the user's active modules.
///
/// Reuses `config::list_active_modules` + `extract_package_names` so
/// the "installed" detection matches what `cheni check` already
/// considers in scope. Falls back to the empty set when the layout
/// is too exotic for the import walker — better silent than wrong.
fn collect_installed_names(cfg: &config::NixConfig) -> HashSet<String> {
    let modules = match config::list_active_modules(&cfg.flake_dir, &cfg.hostname) {
        Some(m) => m,
        None => {
            debug!("no active modules resolved; installed-state badge disabled");
            return HashSet::new();
        }
    };
    config::extract_package_names(&modules).into_iter().collect()
}

/// Look up Repology versions for the top hits. Returns a map keyed by
/// short package name. Empty on any failure mode (offline, rate-limit,
/// Repology outage) — search must keep working.
async fn lookup_upstream(
    rows: &[SearchRow],
) -> std::collections::HashMap<String, String> {
    let to_query: Vec<(String, Option<String>)> = rows
        .iter()
        .take(MAX_REPOLOGY_LOOKUPS)
        .map(|(name, version, _)| (name.clone(), Some(version.clone())))
        .collect();

    if to_query.is_empty() {
        return std::collections::HashMap::new();
    }

    let lookups = match repology::lookup_versions(&to_query).await {
        Ok(v) => v,
        Err(e) => {
            debug!("Repology lookup failed for search: {}", e);
            return std::collections::HashMap::new();
        }
    };

    lookups
        .into_iter()
        .filter_map(|l| l.version.map(|v| (l.name, v)))
        .collect()
}

/// Compose the optional second-line annotation: Repology delta and
/// local-state badges joined by ` · `. Returns `None` when there's
/// nothing worth printing (up-to-date package not in user state).
fn build_annotation(
    name: &str,
    nixpkgs_version: &str,
    upstream: &std::collections::HashMap<String, String>,
    local: &LocalState,
) -> Option<String> {
    let upstream_marker = upstream
        .get(name)
        .filter(|u| repology_differs(nixpkgs_version, u))
        .map(|u| format!("→ {} upstream", u));
    let badge = local.badges(name);

    match (upstream_marker, badge) {
        (None, None) => None,
        (Some(u), None) => Some(u),
        (None, Some(b)) => Some(b),
        (Some(u), Some(b)) => Some(format!("{} · {}", u, b)),
    }
}

/// Decide whether `upstream` is meaningfully different from the
/// nixpkgs `version` for display purposes.
///
/// Returns false when:
/// - either string is empty / "?" placeholder (nothing reliable to compare),
/// - the parsed version vectors compare equal (covers `1.2 == 1.2.0`).
///
/// Anything else is considered different — prerelease vs stable, ahead,
/// behind, calver flips. The badge wording stays neutral (`→ X upstream`)
/// so we don't have to commit to a "newer/older" judgement we'd often
/// get wrong on edge cases (e.g. nixpkgs unstable shipping a hash-only
/// version string).
fn repology_differs(version: &str, upstream: &str) -> bool {
    if version.is_empty() || upstream.is_empty() || version == "?" || upstream == "?" {
        return false;
    }
    let v = crate::version::parse::parse_version(version);
    let u = crate::version::parse::parse_version(upstream);
    if v.is_empty() || u.is_empty() {
        return version != upstream;
    }
    crate::version::compare::compare_versions(&v, &u) != crate::version::compare::VersionDiff::Equal
}

/// Shell out to `nix search nixpkgs <query> --json` and parse the
/// resulting JSON document. The two failure paths (nix invocation,
/// JSON parse) get distinct messages so a bug report is meaningful.
fn run_nix_search(query: &str) -> Result<serde_json::Value> {
    let output = Command::new("nix")
        .args(["search", "nixpkgs", query, "--json"])
        .output()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("nix search failed: {}", stderr.lines().next().unwrap_or(""));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).context("Failed to parse nix search output")
}

/// Flatten the JSON map to (name, version, description) rows, then sort:
/// exact > prefix > substring > other, ties broken alphabetically.
fn parse_and_sort_results(
    obj: &serde_json::Map<String, serde_json::Value>,
    query_lower: &str,
) -> Vec<SearchRow> {
    let mut results: Vec<SearchRow> = obj
        .iter()
        .map(|(full_attr, data)| {
            // full_attr looks like "legacyPackages.x86_64-linux.firefox"
            // — we only want the trailing segment.
            let short_name = full_attr.rsplit('.').next().unwrap_or(full_attr).to_string();
            let version = data
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            let description = data
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (short_name, version, description)
        })
        .collect();

    results.sort_by(|a, b| {
        let rank_a = relevance_rank(&a.0.to_lowercase(), query_lower);
        let rank_b = relevance_rank(&b.0.to_lowercase(), query_lower);
        rank_a.cmp(&rank_b).then_with(|| a.0.cmp(&b.0))
    });
    results
}

/// Annotation indent in the rendered output. Picked so the arrow line
/// sits visually under the version column without lining up rigidly
/// (which would chase ANSI-aware width calculations across rows).
const ANNOT_INDENT: &str = "      ";

fn print_results(
    results: &[SearchRow],
    query_lower: &str,
    local: &LocalState,
    upstream: &std::collections::HashMap<String, String>,
) {
    for (name, version, description) in results.iter().take(MAX_DISPLAY) {
        // Char-based truncation: a byte slice would panic mid-codepoint
        // on a description with an emoji or accented letter at the cut.
        let truncated = if description.chars().count() > 70 {
            let head: String = description.chars().take(67).collect();
            format!("{}...", head)
        } else {
            description.clone()
        };
        // Bold green only for an exact match, so the eye lands there first.
        let name_styled = if name.to_lowercase() == query_lower {
            name.bold().green().to_string()
        } else {
            name.green().to_string()
        };
        println!("  {:<30} {:<14} {}", name_styled, version.dimmed(), truncated);

        if let Some(annot) = build_annotation(name, version, upstream, local) {
            println!("{}{}", ANNOT_INDENT, annot.dimmed());
        }
    }
}

fn print_footer(total: usize) {
    println!();
    if total > MAX_DISPLAY {
        println!("{}", format!("Showing {} of {} results", MAX_DISPLAY, total).dimmed());
    } else {
        println!("{}", format!("{} result(s)", total).dimmed());
    }
}

/// Lower number = more relevant. Used to sort search results so that
/// exact name matches show up first, then prefix matches, then everything
/// else (substring matches and full-text hits from the description).
fn relevance_rank(name_lower: &str, query_lower: &str) -> u8 {
    if name_lower == query_lower {
        0
    } else if name_lower.starts_with(query_lower) {
        1
    } else if name_lower.contains(query_lower) {
        2
    } else {
        3
    }
}

#[cfg(test)]
#[path = "tests/search.rs"]
mod tests;
