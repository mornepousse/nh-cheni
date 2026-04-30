//! `cheni search` command.
//!
//! Searches nixpkgs for packages matching a query, then enriches each
//! displayed hit with cross-context information that `nix search`
//! alone cannot surface:
//!
//! - **Upstream delta** — when the nixpkgs-latest input carries a newer
//!   version than the regular nixpkgs one, the row gets a `→ <version>
//!   upstream` annotation. Helps decide "is this a stale package or
//!   the latest?".
//! - **Local state** — when the package is already in the user's
//!   modules, pinned, or frozen, the row gets the matching badge
//!   (`installed`, `pinned`, `frozen@<v>`).
//!
//! This is the primary cheni vs. nh differentiator: nh has no notion
//! of pins/freezes nor upstream version deltas.

use std::collections::HashSet;
use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use std::path::Path;

use crate::nix::{config, freezes, pins, version_cache};
use crate::nix::eval::lookup_or_eval;
use crate::nix::flake::read_input_locked;
use crate::nix::version_cache::VersionCache;

/// One result row from `nix search`: (short attr name, version, description).
type SearchRow = (String, String, String);

/// Maximum number of search hits we render. Past this threshold, the
/// user is fishing — we'd be making API calls for results they won't
/// look at.
const MAX_DISPLAY: usize = 30;

/// Maximum number of hits we look up against nixpkgs-latest.
///
/// Capped well below `MAX_DISPLAY`: the top-relevance hits are what
/// users actually inspect; deeper hits stay un-annotated and that's
/// fine. Eval calls are cheap (cache-first) but not instantaneous.
const MAX_UPSTREAM_LOOKUPS: usize = 10;

/// Run `cheni search <query>`.
///
/// Uses `nix search nixpkgs <query>` to find matching packages, then
/// evaluates the top hits against nixpkgs-latest for upstream version
/// deltas, and looks up local pin/freeze/installed state. Both
/// enrichments degrade silently on failure — the base `nix search`
/// output always renders.
pub async fn run(query: &str) -> Result<()> {
    println!("{} {}\n", "Searching nixpkgs for".dimmed(), query.bold());

    let raw = run_nix_search(query)?;
    let no_results = || {
        println!("{}", "No packages found.".dimmed());
        println!(
            "  {} try a broader query, check spelling, or browse https://search.nixos.org",
            "·".dimmed()
        );
    };
    let Some(obj) = raw.as_object() else {
        no_results();
        return Ok(());
    };
    if obj.is_empty() {
        no_results();
        return Ok(());
    }
    debug!("Found {} results", obj.len());

    let q = query.to_lowercase();
    let results = parse_and_sort_results(obj, &q);
    let displayed: Vec<SearchRow> = results.iter().take(MAX_DISPLAY).cloned().collect();

    let local = gather_local_state();
    // Detect flake dir so lookup_upstream can read flake.lock and the
    // version cache. Silently skip upstream annotations when absent.
    let flake_dir_opt = config::detect().ok().map(|c| c.flake_dir);
    let upstream = match &flake_dir_opt {
        Some(dir) => lookup_upstream(dir, &displayed).await,
        None => {
            debug!("no flake detected — upstream-delta annotation skipped");
            std::collections::HashMap::new()
        }
    };

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

/// Look up upstream versions for the top hits via nix eval against
/// nixpkgs-latest. Returns a map keyed by short package name.
///
/// Empty on any failure mode (nixpkgs-latest absent, eval error,
/// version cache I/O) — search must keep working without annotations.
async fn lookup_upstream(
    flake_dir: &Path,
    rows: &[SearchRow],
) -> std::collections::HashMap<String, String> {
    let cache_path = version_cache::cache_path();
    let mut cache = match VersionCache::load(&cache_path) {
        Ok(c) => c,
        Err(e) => {
            debug!("version_cache load failed: {e}");
            return std::collections::HashMap::new();
        }
    };
    let Some((rev, nar_hash)) = read_input_locked(flake_dir, "nixpkgs-latest") else {
        // No nixpkgs-latest input — silently return empty so search keeps
        // working without an upstream-delta annotation.
        return std::collections::HashMap::new();
    };

    let mut out = std::collections::HashMap::new();
    for (name, _version, _desc) in rows.iter().take(MAX_UPSTREAM_LOOKUPS) {
        match lookup_or_eval(&mut cache, "nixpkgs-latest", &rev, &nar_hash, name) {
            Ok(Some(v)) => {
                out.insert(name.clone(), v);
            }
            Ok(None) => {} // attr missing or eval gave None
            Err(e) => {
                debug!("lookup_or_eval failed for {name}: {e}");
            }
        }
    }

    if let Err(e) = cache.save(&cache_path) {
        debug!("version_cache save failed: {e}");
    }
    out
}

/// Compose the optional second-line annotation: upstream delta and
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
        .filter(|u| versions_differ(nixpkgs_version, u))
        .map(|u| format!("→ {} upstream", u));
    let badge = local.badges(name);

    match (upstream_marker, badge) {
        (None, None) => None,
        (Some(u), None) => Some(u),
        (None, Some(b)) => Some(b),
        (Some(u), Some(b)) => Some(format!("{} · {}", u, b)),
    }
}

/// Decide whether the upstream version (from the version source) is
/// meaningfully different from the nixpkgs `version` for display
/// purposes.
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
fn versions_differ(version: &str, upstream: &str) -> bool {
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

/// Build the argument list for `nix search`. All real flags (`--json`)
/// stay BEFORE the `--` separator; everything after `--` is positional.
/// This way a query starting with `--` (e.g. `--expr`) lands as the
/// search regex, never as a flag — without poisoning our own `--json`.
fn nix_search_args(query: &str) -> Vec<&str> {
    vec!["search", "nixpkgs", "--json", "--", query]
}

/// Shell out to `nix search nixpkgs <query> --json` and parse the
/// resulting JSON document. The two failure paths (nix invocation,
/// JSON parse) get distinct messages so a bug report is meaningful.
fn run_nix_search(query: &str) -> Result<serde_json::Value> {
    let output = Command::new("nix")
        .args(nix_search_args(query))
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

/// Width of the name column in characters.
const NAME_COL: usize = 30;
/// Width of the version column in characters.
const VER_COL: usize = 14;

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
        // Pad based on the *visible* width (uncolored input) — the
        // built-in `{:<W}` format would count ANSI escapes from
        // `colored` as part of the string length, leaving short names
        // overpadded and 28-char names under-padded.
        println!(
            "  {}{}{}{}{}",
            name_styled,
            pad_to(name, NAME_COL),
            version.dimmed(),
            pad_to(version, VER_COL),
            truncated
        );

        if let Some(annot) = build_annotation(name, version, upstream, local) {
            println!("{}{}", ANNOT_INDENT, annot.dimmed());
        }
    }
}

/// Spaces needed after a column whose visible width is `text.chars().count()`
/// to reach `width`. Always returns at least one space so two adjacent
/// columns never visually merge — even when the content already exceeds
/// the nominal column width.
fn pad_to(text: &str, width: usize) -> String {
    let visible = text.chars().count();
    if visible >= width {
        " ".to_string()
    } else {
        " ".repeat(width - visible)
    }
}

fn print_footer(total: usize) {
    println!();
    if total > MAX_DISPLAY {
        println!("{}", format!("Showing {} of {} results", MAX_DISPLAY, total).dimmed());
    } else {
        println!("{}", crate::util::count_phrase(total, "result").dimmed());
    }
    if total > 0 {
        println!(
            "{}",
            "Tip: pin one with `cheni pin <name>` (newer version via nixpkgs-latest)."
                .dimmed()
        );
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
