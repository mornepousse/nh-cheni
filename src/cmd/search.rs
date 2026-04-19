//! `cheni search` command.
//!
//! Searches nixpkgs for packages matching a query.
//! Uses `nix search` locally — requires nixpkgs to be cached but
//! works offline afterward.

use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

/// One result row from `nix search`: (short attr name, version, description).
type SearchRow = (String, String, String);

const MAX_DISPLAY: usize = 30;

/// Run `cheni search <query>`.
///
/// Uses `nix search nixpkgs <query>` to find matching packages in
/// the user's nixpkgs input.
pub fn run(query: &str) -> Result<()> {
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
    print_results(&results, &q);
    print_footer(results.len());
    Ok(())
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

fn print_results(results: &[SearchRow], query_lower: &str) {
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
