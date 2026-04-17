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
        .context("Failed to run 'nix search'. Is nix installed?")?;

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

    results.sort_by(|a, b| a.0.cmp(&b.0));

    // Display (cap at 30 results)
    let max_display = 30;
    for (name, version, description) in results.iter().take(max_display) {
        let truncated = if description.len() > 70 {
            format!("{}...", &description[..67])
        } else {
            description.clone()
        };

        println!(
            "  {:<30} {:<14} {}",
            name.green(),
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
