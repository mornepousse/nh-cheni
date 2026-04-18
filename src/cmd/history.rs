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

    // Per-package details: (name, "old → new" or None for rebuilt/added/removed)
    let mut updated: Vec<(String, String)> = Vec::new(); // (name, "old → new")
    let mut added: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut rebuilt: Vec<String> = Vec::new();
    let mut size_delta_kib: f64 = 0.0;

    for line in stdout.lines() {
        let clean = strip_ansi(line);
        let trimmed = clean.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(delta) = parse_size_delta(trimmed) {
            size_delta_kib += delta;
        }

        // Each line starts with "name: " followed by the change description.
        let (name, rest) = match trimmed.split_once(": ") {
            Some(p) => p,
            None => continue,
        };
        let name = name.trim().to_string();

        // `nix store diff-closures` per-line formats:
        //   "1.0 → 2.0[, +size]"   version change
        //   "∅ → ε" / "∅ → 1.0"     added
        //   "ε → ∅" / "1.0 → ∅"     removed
        //   "38.6 KiB"             same version, closure rebuilt
        if rest.contains("∅ →") || rest.contains("∅ ->") {
            added.push(name);
        } else if rest.contains("→ ∅") || rest.contains("-> ∅") {
            removed.push(name);
        } else if rest.contains(" → ") || rest.contains(" -> ") {
            // Extract the version transition (everything up to first comma if any).
            let versions = rest.split(',').next().unwrap_or(rest).trim().to_string();
            updated.push((name, versions));
        } else {
            rebuilt.push(name);
        }
    }

    if updated.is_empty() && added.is_empty() && removed.is_empty() && rebuilt.is_empty() {
        return Some("(identical closures)".to_string());
    }

    let mut parts = Vec::new();
    if !updated.is_empty() {
        parts.push(format_update_list(&updated));
    }
    if !added.is_empty() {
        parts.push(format!("+ {}", format_name_list(&added)));
    }
    if !removed.is_empty() {
        parts.push(format!("- {}", format_name_list(&removed)));
    }
    if !rebuilt.is_empty() {
        parts.push(format!("⟳ {}", format_name_list(&rebuilt)));
    }
    if size_delta_kib.abs() >= 0.1 {
        parts.push(format_size_delta(size_delta_kib));
    }
    Some(parts.join(", "))
}

/// Format an update list with versions if there's a single one,
/// otherwise list names compactly: "↑ claude-code (2.1.113 → 2.1.114)"
/// or "↑ foo, bar (+2 more)".
fn format_update_list(updates: &[(String, String)]) -> String {
    if updates.len() == 1 {
        format!("↑ {} ({})", updates[0].0, updates[0].1)
    } else {
        let names: Vec<&str> = updates.iter().map(|(n, _)| n.as_str()).collect();
        format!("↑ {}", join_with_overflow(&names, 3))
    }
}

/// Join package names: first N then "(+K more)" if longer.
fn format_name_list(names: &[String]) -> String {
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    join_with_overflow(&refs, 3)
}

fn join_with_overflow(items: &[&str], max: usize) -> String {
    if items.len() <= max {
        items.join(", ")
    } else {
        let head = items[..max].join(", ");
        format!("{} (+{} more)", head, items.len() - max)
    }
}

/// Strip ANSI escape sequences (CSI codes) from a line.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            // Skip until a letter (final byte of CSI sequence)
            while let Some(&n) = chars.peek() {
                chars.next();
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse a "+/-N.N KiB" or "N.N MiB" size delta from a diff-closures line.
/// Returns the value normalised to KiB (positive = added, negative = removed).
fn parse_size_delta(line: &str) -> Option<f64> {
    // Find the last token that looks like "<number> <unit>"
    // e.g. "cheni: 38.6 KiB", "pkg: 2.0 → 3.0, 1.2 MiB", "pkg: -512.0 KiB"
    for unit in &["KiB", "MiB", "GiB"] {
        if let Some(idx) = line.rfind(unit) {
            let before = &line[..idx].trim_end();
            // Walk back to find the number
            let num_start = before.rfind(|c: char| c == ' ' || c == ',').map(|i| i + 1).unwrap_or(0);
            let num_str = before[num_start..].trim();
            if let Ok(n) = num_str.parse::<f64>() {
                let kib = match *unit {
                    "KiB" => n,
                    "MiB" => n * 1024.0,
                    "GiB" => n * 1024.0 * 1024.0,
                    _ => n,
                };
                return Some(kib);
            }
        }
    }
    None
}

/// Format a size delta in KiB to a short human-readable string ("+38 KiB", "-1.2 MiB").
fn format_size_delta(kib: f64) -> String {
    let sign = if kib >= 0.0 { "+" } else { "-" };
    let abs = kib.abs();
    if abs < 1024.0 {
        format!("{}{:.0} KiB", sign, abs)
    } else if abs < 1024.0 * 1024.0 {
        format!("{}{:.1} MiB", sign, abs / 1024.0)
    } else {
        format!("{}{:.1} GiB", sign, abs / (1024.0 * 1024.0))
    }
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
