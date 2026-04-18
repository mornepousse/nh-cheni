//! `cheni search` command.
//!
//! Searches nixpkgs for packages matching a query.
//! Uses `nix search` locally — requires nixpkgs to be cached but
//! works offline afterward.

use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

/// Run `cheni search <query>`.
///
/// Uses `nix search nixpkgs <query>` to find matching packages in
/// the user's nixpkgs input.
pub fn run(query: &str) -> Result<()> {
    println!(
        "{} {}\n",
        "Searching nixpkgs for".dimmed(),
        query.bold()
    );

    // Use `nix search` with JSON output for parsing
    let output = Command::new("nix")
        .args(["search", "nixpkgs", query, "--json"])
        .output()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("nix search failed: {}", stderr.lines().next().unwrap_or(""));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .context("Failed to parse nix search output")?;

    let obj = match json.as_object() {
        Some(o) => o,
        None => {
            println!("{}", "No packages found.".dimmed());
            return Ok(());
        }
    };

    if obj.is_empty() {
        println!("{}", "No packages found.".dimmed());
        return Ok(());
    }

    debug!("Found {} results", obj.len());

    // Collect and sort results by attr name
    let mut results: Vec<(String, String, String)> = obj.iter().map(|(full_attr, data)| {
        // full_attr is like "legacyPackages.x86_64-linux.firefox"
        // We want just "firefox"
        let short_name = full_attr.rsplit('.').next().unwrap_or(full_attr).to_string();

        let version = data.get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();

        let description = data.get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        (short_name, version, description)
    }).collect();

    // Sort by relevance: exact matches first, then prefix matches,
    // then substring matches, then alphabetical within each bucket.
    let q = query.to_lowercase();
    results.sort_by(|a, b| {
        let rank_a = relevance_rank(&a.0.to_lowercase(), &q);
        let rank_b = relevance_rank(&b.0.to_lowercase(), &q);
        rank_a.cmp(&rank_b).then_with(|| a.0.cmp(&b.0))
    });

    // Display (cap at 30 results)
    let max_display = 30;
    for (name, version, description) in results.iter().take(max_display) {
        let truncated = if description.len() > 70 {
            format!("{}...", &description[..67])
        } else {
            description.clone()
        };

        // Highlight the matching package name in green; perfect matches
        // in bold green so the eye lands there first.
        let name_styled = if name.to_lowercase() == q {
            name.bold().green().to_string()
        } else {
            name.green().to_string()
        };

        println!(
            "  {:<30} {:<14} {}",
            name_styled,
            version.dimmed(),
            truncated,
        );
    }

    let total = results.len();
    println!();
    if total > max_display {
        println!(
            "{}",
            format!("Showing {} of {} results", max_display, total).dimmed()
        );
    } else {
        println!("{}", format!("{} result(s)", total).dimmed());
    }

    Ok(())
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
