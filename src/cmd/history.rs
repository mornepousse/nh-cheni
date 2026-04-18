//! `cheni history` command.
//!
//! Lists all NixOS system generations with their date, kernel,
//! and the differences (added/changed/removed packages).
//!
//! Also handles selective generation deletion via the `--prune`,
//! `--delete`, `--keep`, and `--older-than` flags.

use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, MultiSelect};
use tracing::debug;

/// Options accepted by `cheni history`.
pub struct HistoryOptions {
    pub diff: bool,
    /// Show the full summary even if it doesn't fit on one line.
    pub full: bool,
    pub limit: Option<usize>,
    /// Specific generation numbers or ranges to delete.
    pub delete: Vec<String>,
    /// Pick generations to delete interactively.
    pub prune: bool,
    /// Delete the oldest generations, keeping only N most recent.
    pub keep: Option<usize>,
    /// Delete generations older than this duration spec (e.g. "30d").
    pub older_than: Option<String>,
    /// Run nix-collect-garbage after deletion.
    pub gc: bool,
    /// Skip confirmation prompt.
    pub yes: bool,
}

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
/// Use --prune / --delete / --keep / --older-than to remove generations.
pub fn run(opts: HistoryOptions) -> Result<()> {
    let generations = read_generations()?;

    if generations.is_empty() {
        println!("{}\n", "=== cheni history ===".bold());
        println!("{}", "No generations found.".dimmed());
        println!("  This requires read access to /nix/var/nix/profiles/system-*-link");
        return Ok(());
    }

    let in_delete_mode = opts.prune
        || !opts.delete.is_empty()
        || opts.keep.is_some()
        || opts.older_than.is_some();

    if in_delete_mode {
        return run_delete(&opts, &generations);
    }

    println!("{}\n", "=== cheni history ===".bold());

    let total = generations.len();
    let to_show = opts.limit.unwrap_or(10).min(total);

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

            if opts.diff {
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
                // Compact summary. Default truncates to terminal width;
                // --full keeps the whole thing.
                if let Some(summary) = get_diff_summary(&previous.store_path, &gen.store_path) {
                    let display = if opts.full {
                        summary
                    } else {
                        truncate_to_terminal(&summary, 6) // 6 = "      " indent
                    };
                    println!("      {}", display.dimmed());
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

    if !opts.full {
        println!(
            "{}",
            "Tip: pass --full to see the complete summary, --diff for the per-package nvd output."
                .dimmed()
        );
    }

    Ok(())
}

/// Truncate `s` so that, prefixed by `indent` spaces, it fits the terminal
/// width. Adds a trailing " …" marker when truncation happens.
///
/// Width is taken from the TIOCGWINSZ ioctl when stdout is a TTY, otherwise
/// from $COLUMNS (handy in pipes / scripts), otherwise no truncation occurs.
fn truncate_to_terminal(s: &str, indent: usize) -> String {
    let cols = terminal_size::terminal_size()
        .map(|(terminal_size::Width(w), _)| w as usize)
        .or_else(|| std::env::var("COLUMNS").ok().and_then(|v| v.parse().ok()));

    let cols = match cols {
        Some(c) => c,
        None => return s.to_string(),
    };

    let budget = cols.saturating_sub(indent);
    if s.chars().count() <= budget {
        return s.to_string();
    }
    let suffix = " …";
    let take = budget.saturating_sub(suffix.chars().count());
    let mut out: String = s.chars().take(take).collect();
    // Avoid cutting in the middle of a word if there's a recent space
    if let Some(last_space) = out.rfind(' ') {
        if out.len() - last_space < 12 {
            out.truncate(last_space);
        }
    }
    out.push_str(suffix);
    out
}

/// Resolve which generations to delete from the user's flags, then run
/// `nix-env --delete-generations` (with sudo) for the chosen numbers.
fn run_delete(opts: &HistoryOptions, generations: &[Generation]) -> Result<()> {
    println!("{}\n", "=== cheni history (prune) ===".bold());

    let all: Vec<u32> = generations.iter().map(|g| g.number).collect();
    let current = generations.iter().find(|g| g.is_current).map(|g| g.number);

    let mut to_delete: Vec<u32> = Vec::new();

    if opts.prune {
        to_delete.extend(pick_interactively(generations, current)?);
    }

    for spec in &opts.delete {
        to_delete.extend(parse_target_spec(spec, &all)?);
    }

    if let Some(k) = opts.keep {
        to_delete.extend(pick_oldest_beyond(&all, k));
    }

    if let Some(spec) = opts.older_than.as_deref() {
        let days = parse_duration_days(spec)
            .with_context(|| format!("Invalid --older-than value: '{}'", spec))?;
        to_delete.extend(pick_older_than(&all, days)?);
    }

    to_delete.sort_unstable();
    to_delete.dedup();

    // Refuse to delete the active generation — it would brick rollback.
    if let Some(c) = current {
        if to_delete.contains(&c) {
            anyhow::bail!(
                "Refusing to delete the active generation ({}). \
                 Switch to another generation first (cheni rollback).",
                c
            );
        }
    }

    if to_delete.is_empty() {
        println!("{}", "Nothing to delete.".dimmed());
        return Ok(());
    }

    println!(
        "Will delete {} generation(s):",
        to_delete.len().to_string().bold()
    );
    for n in &to_delete {
        println!("  {} {}", "-".red(), n.to_string().bold());
    }
    println!();

    if !opts.yes {
        let confirm = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Proceed?")
            .default(false)
            .interact()?;
        if !confirm {
            println!("{}", "Aborted.".dimmed());
            return Ok(());
        }
    }

    let mut args: Vec<String> = vec![
        "/run/current-system/sw/bin/nix-env".to_string(),
        "-p".to_string(),
        "/nix/var/nix/profiles/system".to_string(),
        "--delete-generations".to_string(),
    ];
    args.extend(to_delete.iter().map(|n| n.to_string()));

    println!("{}", "Requires sudo to modify the system profile.".dimmed());
    let status = Command::new("sudo")
        .args(&args)
        .status()
        .context("Failed to run nix-env --delete-generations")?;

    if !status.success() {
        anyhow::bail!("Generation deletion failed");
    }

    println!(
        "\n{} {} generation(s) removed.",
        "✓".green(),
        to_delete.len()
    );

    if opts.gc {
        println!("\n{}", "Running garbage collection...".bold());
        let gc_status = Command::new("sudo")
            .args(["/run/current-system/sw/bin/nix-collect-garbage"])
            .status()
            .context("Failed to run nix-collect-garbage")?;
        if !gc_status.success() {
            anyhow::bail!("Garbage collection failed");
        }
        println!("\n{} Disk space reclaimed.", "✓".green());
    } else {
        println!(
            "{}",
            "  (store paths kept until next GC — pass --gc to reclaim disk now)".dimmed()
        );
    }

    Ok(())
}

/// Parse a target spec string into a list of generation numbers.
/// Accepts "405", "405..410" (inclusive range).
fn parse_target_spec(spec: &str, all: &[u32]) -> Result<Vec<u32>> {
    if let Some((from, to)) = spec.split_once("..") {
        let from: u32 = from
            .parse()
            .with_context(|| format!("Invalid range start in '{}'", spec))?;
        let to: u32 = to
            .parse()
            .with_context(|| format!("Invalid range end in '{}'", spec))?;
        let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
        Ok(all.iter().copied().filter(|n| *n >= lo && *n <= hi).collect())
    } else {
        let n: u32 = spec
            .parse()
            .with_context(|| format!("Invalid generation number '{}'", spec))?;
        if !all.contains(&n) {
            anyhow::bail!("Generation {} does not exist", n);
        }
        Ok(vec![n])
    }
}

/// Return all generations except the `keep` most recent.
fn pick_oldest_beyond(all: &[u32], keep: usize) -> Vec<u32> {
    if all.len() <= keep {
        return Vec::new();
    }
    all[..all.len() - keep].to_vec()
}

/// Parse a duration like "30d", "2w", "1m" into days.
fn parse_duration_days(spec: &str) -> Result<u64> {
    let spec = spec.trim();
    let (num_part, unit) = spec.split_at(
        spec.find(|c: char| !c.is_ascii_digit())
            .unwrap_or(spec.len()),
    );
    let n: u64 = num_part
        .parse()
        .with_context(|| format!("Expected a number, got '{}'", num_part))?;
    let multiplier = match unit.trim() {
        "" | "d" => 1,
        "w" => 7,
        "m" => 30,
        "y" => 365,
        other => anyhow::bail!("Unknown time unit '{}' (use d, w, m, y)", other),
    };
    Ok(n * multiplier)
}

/// Pick generations whose symlink mtime is older than `days` days.
fn pick_older_than(all: &[u32], days: u64) -> Result<Vec<u32>> {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(days * 86400))
        .context("Cutoff date underflow")?;

    let mut out = Vec::new();
    for &n in all {
        let path = format!("/nix/var/nix/profiles/system-{}-link", n);
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if let Ok(modified) = meta.modified() {
            if modified < cutoff {
                out.push(n);
            }
        }
    }
    Ok(out)
}

/// Show a multi-select picker so the user can tick generations to delete.
/// The active generation is shown but excluded from the result.
fn pick_interactively(generations: &[Generation], current: Option<u32>) -> Result<Vec<u32>> {
    // Newest first for picking
    let ordered: Vec<&Generation> = generations.iter().rev().collect();

    let labels: Vec<String> = ordered
        .iter()
        .map(|g| {
            let marker = if Some(g.number) == current { " (current)" } else { "" };
            let summary = if let Some(idx) = ordered.iter().position(|x| x.number == g.number) {
                if idx + 1 < ordered.len() {
                    let prev = ordered[idx + 1];
                    get_diff_summary(&prev.store_path, &g.store_path)
                        .unwrap_or_default()
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            let summary_str = if summary.is_empty() {
                String::new()
            } else {
                format!("  — {}", summary)
            };
            format!("{:<5} {}{}{}", g.number, g.date, marker, summary_str)
        })
        .collect();

    let selection = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Pick generations to delete (space = toggle, enter = confirm)")
        .items(&labels)
        .interact_opt()?
        .unwrap_or_default();

    Ok(selection
        .into_iter()
        .map(|i| ordered[i].number)
        .filter(|n| Some(*n) != current)
        .collect())
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
    summarize_diff(&stdout)
}

/// Pure parsing half of `get_diff_summary` — takes the raw stdout of
/// `nix store diff-closures` and turns it into the one-line human
/// summary. Extracted so it can be exercised with fixtures instead of
/// needing two real store paths.
///
/// `nix store diff-closures` per-line formats we've observed:
///   "pkg: 1.0 → 2.0"                 version change
///   "pkg: 1.0 → 2.0, +size"          version change + size delta
///   "pkg: ∅ → ε" / "pkg: ∅ → 1.0"     added (with/without version)
///   "pkg: ε → ∅" / "pkg: 1.0 → ∅"     removed (with/without version)
///   "pkg: 38.6 KiB" (ANSI-wrapped)   same version, closure rebuilt
///
/// ANSI colour codes are stripped up front because nix colours the
/// size delta in red-bold by default.
fn summarize_diff(stdout: &str) -> Option<String> {
    let mut updated: Vec<(String, String)> = Vec::new();
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

        let (name, rest) = match trimmed.split_once(": ") {
            Some(p) => p,
            None => continue,
        };
        let name = name.trim().to_string();

        if rest.contains("∅ →") || rest.contains("∅ ->") {
            added.push(name);
        } else if rest.contains("→ ∅") || rest.contains("-> ∅") {
            removed.push(name);
        } else if rest.contains(" → ") || rest.contains(" -> ") {
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
            let num_start = before.rfind([' ', ',']).map(|i| i + 1).unwrap_or(0);
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

#[cfg(test)]
mod diff_parser_tests {
    //! Regression tests for summarize_diff — protects against format drift
    //! in `nix store diff-closures` output. Each fixture is a raw stdout
    //! sample (anonymised) from real diffs observed during cheni development.
    use super::*;

    #[test]
    fn identical_closures() {
        // Empty stdout = nothing changed between the two generations.
        assert_eq!(summarize_diff(""), Some("(identical closures)".to_string()));
    }

    #[test]
    fn single_version_bump() {
        let out = "claude-code: 2.1.113 → 2.1.114";
        let s = summarize_diff(out).unwrap();
        assert!(s.contains("↑ claude-code"), "got: {}", s);
        assert!(s.contains("2.1.113 → 2.1.114"), "got: {}", s);
    }

    #[test]
    fn version_bump_with_size_delta() {
        // Real observed form: nix appends ", +NNN KiB" in red ANSI after
        // a version bump that brought in a larger derivation.
        let out = "claude-code: 2.1.112 → 2.1.113, \x1b[31;1m552.0 KiB\x1b[0m";
        let s = summarize_diff(out).unwrap();
        assert!(s.contains("↑ claude-code (2.1.112 → 2.1.113)"), "got: {}", s);
        assert!(s.contains("+552 KiB"), "got: {}", s);
    }

    #[test]
    fn rebuild_only_size_delta() {
        // Same version, closure content changed (e.g. cheni rebuilt from
        // a new source). Pure size line with no arrow.
        let out = "cheni: \x1b[31;1m38.6 KiB\x1b[0m";
        let s = summarize_diff(out).unwrap();
        assert!(s.contains("⟳ cheni"), "got: {}", s);
        assert!(s.contains("+39 KiB"), "got: {}", s);
    }

    #[test]
    fn added_and_removed() {
        // ∅ → ε marks a new derivation with no version; ε → ∅ marks
        // a removal. Both appear for unit-file renames during rebuilds.
        let out = "hm_nviminit.lua: ∅ → ε\nwrapper-init: ε → ∅";
        let s = summarize_diff(out).unwrap();
        assert!(s.contains("+ hm_nviminit.lua"), "got: {}", s);
        assert!(s.contains("- wrapper-init"), "got: {}", s);
    }

    #[test]
    fn removed_with_version() {
        let out = "old-pkg: 1.2.3 → ∅";
        let s = summarize_diff(out).unwrap();
        assert!(s.contains("- old-pkg"), "got: {}", s);
    }

    #[test]
    fn added_with_version() {
        let out = "new-pkg: ∅ → 2.0.0";
        let s = summarize_diff(out).unwrap();
        assert!(s.contains("+ new-pkg"), "got: {}", s);
    }

    #[test]
    fn big_upgrade_truncates_to_three_plus_more() {
        // 5 updates → shows first 3 names then "(+2 more)" marker.
        let out = "\
a: 1.0 → 2.0\n\
b: 1.0 → 2.0\n\
c: 1.0 → 2.0\n\
d: 1.0 → 2.0\n\
e: 1.0 → 2.0\n";
        let s = summarize_diff(out).unwrap();
        assert!(s.starts_with("↑ a, b, c"), "got: {}", s);
        assert!(s.contains("(+2 more)"), "got: {}", s);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        // Lines without "name: " aren't valid diff entries — must not panic
        // or produce bogus entries.
        let out = "\
some banner line\n\
claude-code: 1.0 → 2.0\n\
another weird line\n\
---\n";
        let s = summarize_diff(out).unwrap();
        // Only the real line is picked up.
        assert!(s.contains("↑ claude-code"), "got: {}", s);
    }

    #[test]
    fn ascii_arrow_fallback_is_parsed() {
        // Locales without Unicode sometimes emit "->" instead of "→".
        // The parser accepts both so we don't silently lose entries.
        let out = "foo: 1.0 -> 2.0";
        let s = summarize_diff(out).unwrap();
        assert!(s.contains("↑ foo"), "got: {}", s);
    }

    #[test]
    fn mib_size_delta_is_aggregated() {
        // Two rebuilt packages contributing MiB — aggregated into one
        // suffix on the summary line.
        let out = "\
kernel: \x1b[31;1m45.2 MiB\x1b[0m\n\
firefox: \x1b[31;1m33.4 MiB\x1b[0m\n";
        let s = summarize_diff(out).unwrap();
        assert!(s.contains("⟳"), "got: {}", s);
        // 45.2 + 33.4 = 78.6 MiB
        assert!(s.contains("+78.6 MiB") || s.contains("+78 MiB"), "got: {}", s);
    }

    #[test]
    fn strip_ansi_leaves_plain_text_alone() {
        assert_eq!(strip_ansi("hello world"), "hello world");
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("\x1b[31;1mred-bold\x1b[0m stays"), "red-bold stays");
    }

    #[test]
    fn parse_size_delta_variants() {
        assert_eq!(parse_size_delta("cheni: 38.6 KiB"), Some(38.6));
        assert_eq!(parse_size_delta("kernel: 45.2 MiB"), Some(45.2 * 1024.0));
        assert_eq!(parse_size_delta("nothing here"), None);
        assert_eq!(parse_size_delta(""), None);
    }
}
