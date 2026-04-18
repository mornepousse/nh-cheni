//! `cheni history` command.
//!
//! Lists all NixOS system generations with their date, kernel,
//! and the differences (added/changed/removed packages).

use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

/// A single NixOS generation.
struct Generation {
    /// Generation number.
    number: u32,
    /// Date the generation was created (human readable).
    date: String,
    /// Whether this is the currently active generation.
    is_current: bool,
    /// Path to the generation in the store.
    store_path: String,
}

/// Run `cheni history`.
///
/// Lists all system generations with their differences.
/// Use --diff to show package changes between generations.
pub fn run(diff: bool, limit: Option<usize>) -> Result<()> {
    println!("{}\n", "=== cheni history ===".bold());

    let generations = read_generations()?;

    if generations.is_empty() {
        println!("{}", "No generations found.".dimmed());
        println!(
            "  This requires read access to /nix/var/nix/profiles/system-*-link"
        );
        return Ok(());
    }

    let total = generations.len();
    let to_show = limit.unwrap_or(10).min(total);

    // Show most recent first
    let displayed: Vec<&Generation> = generations.iter().rev().take(to_show).collect();

    for (i, gen) in displayed.iter().enumerate() {
        let marker = if gen.is_current {
            "●".green().to_string()
        } else {
            "○".dimmed().to_string()
        };

        let label = if gen.is_current {
            format!("Generation {} (current)", gen.number).bold().green().to_string()
        } else {
            format!("Generation {}", gen.number).bold().to_string()
        };

        println!("  {} {}", marker, label);
        println!("    {} {}", "Date:".dimmed(), gen.date);

        // If diff requested AND there's a previous generation, show changes
        if diff && i + 1 < displayed.len() {
            let previous = displayed[i + 1];
            println!("    {} computing...", "Diff:".dimmed());

            match get_diff(&previous.store_path, &gen.store_path) {
                Ok(diff_text) if !diff_text.is_empty() => {
                    // Print diff indented
                    for line in diff_text.lines() {
                        println!("      {}", line);
                    }
                }
                Ok(_) => {
                    println!("      {}", "(no version changes)".dimmed());
                }
                Err(_) => {
                    println!("      {}", "(diff unavailable)".dimmed());
                }
            }
        }

        println!();
    }

    if total > to_show {
        println!(
            "{}",
            format!("Showing {} most recent of {} generations. Use --limit N to see more.", to_show, total).dimmed()
        );
    } else {
        println!("{}", format!("{} generation(s) total", total).dimmed());
    }

    Ok(())
}

/// Read all system generations by listing symlinks in /nix/var/nix/profiles.
fn read_generations() -> Result<Vec<Generation>> {
    let profiles_dir = std::path::Path::new("/nix/var/nix/profiles");

    // Find current generation (target of "system" symlink)
    let current_target = std::fs::read_link(profiles_dir.join("system")).ok();
    let current_num = current_target
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .and_then(|s| {
            // "system-407-link" -> 407
            s.strip_prefix("system-")?
                .strip_suffix("-link")?
                .parse::<u32>()
                .ok()
        });

    let entries = std::fs::read_dir(profiles_dir)
        .context("Cannot read /nix/var/nix/profiles")?;

    let mut generations = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => continue,
        };

        // Match "system-<NUM>-link"
        let number = match name_str
            .strip_prefix("system-")
            .and_then(|s| s.strip_suffix("-link"))
            .and_then(|s| s.parse::<u32>().ok())
        {
            Some(n) => n,
            None => continue,
        };

        // Get the modification time of the symlink for the date
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        let date = metadata
            .modified()
            .ok()
            .and_then(|t| {
                let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
                Some(format_unix_date(secs))
            })
            .unwrap_or_else(|| "?".to_string());

        let store_path = entry.path().to_string_lossy().to_string();

        generations.push(Generation {
            number,
            date,
            is_current: current_num == Some(number),
            store_path,
        });
    }

    // Sort by generation number
    generations.sort_by_key(|g| g.number);

    debug!("Found {} generations", generations.len());
    Ok(generations)
}

/// Format a unix timestamp as "YYYY-MM-DD HH:MM" (UTC).
fn format_unix_date(secs: u64) -> String {
    // Simple date formatting without chrono — UTC, basic
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;

    // Convert days since 1970-01-01 to Y/M/D (Howard Hinnant's algorithm)
    let z = days_since_epoch as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, hours, minutes)
}

/// Compute a diff between two generations using nvd if available.
fn get_diff(from: &str, to: &str) -> Result<String> {
    // Try nvd first (much nicer output)
    let nvd = Command::new("nvd")
        .args(["diff", from, to])
        .output();

    if let Ok(o) = nvd {
        if o.status.success() {
            let stdout = String::from_utf8_lossy(&o.stdout);
            return Ok(stdout.to_string());
        }
    }

    // Fallback: nix store diff-closures
    let nix_diff = Command::new("nix")
        .args(["store", "diff-closures", from, to])
        .output()
        .context("Neither 'nvd' nor 'nix store diff-closures' available")?;

    let stdout = String::from_utf8_lossy(&nix_diff.stdout);
    Ok(stdout.to_string())
}
