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
    /// NixOS version label (e.g. "26.05.20260414.4bd9165").
    nixos_label: Option<String>,
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

        // Compact NixOS label: extract just date + commit
        // e.g. "26.05.20260414.4bd9165" → "20260414.4bd9165"
        let label_short = gen.nixos_label.as_deref().map(|l| {
            let parts: Vec<&str> = l.splitn(3, '.').collect();
            if parts.len() == 3 {
                parts[2].to_string()
            } else {
                l.to_string()
            }
        }).unwrap_or_else(|| "?".to_string());

        println!(
            "  {} {}  {}  {}",
            marker,
            label,
            gen.date.dimmed(),
            label_short.cyan(),
        );

        // Show summary diff vs previous generation (always, not just with --diff)
        if i + 1 < displayed.len() {
            let previous = displayed[i + 1];

            if diff {
                // Full diff requested
                match get_diff(&previous.store_path, &gen.store_path) {
                    Ok(diff_text) if !diff_text.is_empty() => {
                        for line in diff_text.lines() {
                            println!("      {}", line.dimmed());
                        }
                    }
                    Ok(_) => {
                        println!("      {}", "(no version changes)".dimmed());
                    }
                    Err(_) => {
                        println!("      {}", "(diff unavailable)".dimmed());
                    }
                }
            } else {
                // Compact summary: count changes
                if let Some(summary) = get_diff_summary(&previous.store_path, &gen.store_path) {
                    println!("      {}", summary.dimmed());
                }
            }
        }
    }

    println!();

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

        // Read the symlink target to extract the NixOS label
        // Target looks like: /nix/store/abc-nixos-system-morthinkpad-26.05.20260414.4bd9165
        let nixos_label = std::fs::read_link(entry.path())
            .ok()
            .and_then(|target| {
                let target_str = target.to_string_lossy().to_string();
                // Extract the version part after "nixos-system-<hostname>-"
                target_str
                    .rsplit('/')
                    .next()
                    .and_then(|name| name.split_once("nixos-system-"))
                    .and_then(|(_, rest)| {
                        // rest = "morthinkpad-26.05.20260414.4bd9165"
                        rest.split_once('-')
                            .map(|(_, version)| version.to_string())
                    })
            });

        generations.push(Generation {
            number,
            date,
            is_current: current_num == Some(number),
            store_path,
            nixos_label,
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

/// Get a compact one-line summary of changes between two generations.
/// Returns something like "↑ 5 updated, + 2 added, - 1 removed".
fn get_diff_summary(from: &str, to: &str) -> Option<String> {
    let output = Command::new("nix")
        .args(["store", "diff-closures", from, to])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut updated = 0;
    let mut added = 0;
    let mut removed = 0;

    for line in stdout.lines() {
        let trimmed = line.trim();
        // nix store diff-closures format:
        // "package: 1.0 -> 2.0"     (update)
        // "package: ∅ -> 1.0"        (added)
        // "package: 1.0 -> ∅"        (removed)
        if trimmed.contains("∅ →") || trimmed.contains("∅ ->") {
            added += 1;
        } else if trimmed.contains("→ ∅") || trimmed.contains("-> ∅") {
            removed += 1;
        } else if trimmed.contains(" → ") || trimmed.contains(" -> ") {
            updated += 1;
        }
    }

    if updated == 0 && added == 0 && removed == 0 {
        return Some("(no changes)".to_string());
    }

    let mut parts = Vec::new();
    if updated > 0 {
        parts.push(format!("↑ {} updated", updated));
    }
    if added > 0 {
        parts.push(format!("+ {} added", added));
    }
    if removed > 0 {
        parts.push(format!("- {} removed", removed));
    }
    Some(parts.join(", "))
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
