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
        print_generation_header(gen);
        if i + 1 < displayed.len() {
            let previous = displayed[i + 1];
            print_generation_diff(previous, gen, opts.diff, opts.full);
        }
    }

    println!();
    print_history_footer(total, to_show, opts.full);
    Ok(())
}

/// One-line header per generation: marker, label, date, short nixpkgs commit.
fn print_generation_header(gen: &Generation) {
    let marker = if gen.is_current {
        "●".green().to_string()
    } else {
        "○".dimmed().to_string()
    };
    let label = if gen.is_current {
        format!("Generation {} (current)", gen.number)
            .bold()
            .green()
            .to_string()
    } else {
        format!("Generation {}", gen.number).bold().to_string()
    };
    // "26.05.20260414.4bd9165" → "20260414.4bd9165"
    let label_short = gen
        .nixos_label
        .as_deref()
        .map(|l| {
            let parts: Vec<&str> = l.splitn(3, '.').collect();
            if parts.len() == 3 { parts[2].to_string() } else { l.to_string() }
        })
        .unwrap_or_else(|| "?".to_string());
    println!(
        "  {} {}  {}  {}",
        marker,
        label,
        gen.date.dimmed(),
        label_short.cyan(),
    );
}

/// Indented diff block under a generation header. With `--diff`, prints
/// the full nvd / diff-closures output; otherwise the one-line compact
/// summary, truncated to the terminal width unless `--full`.
fn print_generation_diff(previous: &Generation, current: &Generation, full_diff: bool, full_summary: bool) {
    if full_diff {
        match get_diff(&previous.store_path, &current.store_path) {
            Ok(diff_text) if !diff_text.is_empty() => {
                for line in diff_text.lines() {
                    println!("      {}", line.dimmed());
                }
            }
            Ok(_) => println!("      {}", "(no version changes)".dimmed()),
            Err(_) => println!("      {}", "(diff unavailable)".dimmed()),
        }
        return;
    }
    if let Some(summary) = get_diff_summary(&previous.store_path, &current.store_path) {
        let display = if full_summary {
            summary
        } else {
            truncate_to_terminal(&summary, 6) // 6 = "      " indent
        };
        println!("      {}", display.dimmed());
    }
}

/// Bottom note: "showing N of M" + the --full / --diff tip.
fn print_history_footer(total: usize, shown: usize, full: bool) {
    if total > shown {
        println!(
            "{}",
            format!(
                "Showing {} most recent of {} generations. Use --limit N to see more.",
                shown, total
            )
            .dimmed()
        );
    } else {
        println!("{}", format!("{} generation(s) total", total).dimmed());
    }
    if !full {
        println!(
            "{}",
            "Tip: pass --full to see the complete summary, --diff for the per-package nvd output."
                .dimmed()
        );
    }
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

/// Top-level dispatcher for `cheni history --prune/--delete/--keep/--older-than`.
/// Reads as four phases: collect targets, guard the active gen, confirm,
/// then apply.
fn run_delete(opts: &HistoryOptions, generations: &[Generation]) -> Result<()> {
    println!("{}\n", "=== cheni history (prune) ===".bold());

    let current = generations.iter().find(|g| g.is_current).map(|g| g.number);
    let to_delete = collect_delete_targets(opts, generations, current)?;
    if to_delete.is_empty() {
        println!("{}", "Nothing to delete.".dimmed());
        return Ok(());
    }
    if !confirm_targets(&to_delete, opts.yes)? {
        return Ok(());
    }
    apply_deletion(&to_delete)?;
    if opts.gc {
        run_gc()?;
    } else {
        println!(
            "{}",
            "  (store paths kept until next GC — pass --gc to reclaim disk now)".dimmed()
        );
    }
    Ok(())
}

/// Resolve every selection flag into a deduplicated list of generation
/// numbers. Bails if the active generation ends up in the set —
/// deleting it would brick `cheni rollback`.
fn collect_delete_targets(
    opts: &HistoryOptions,
    generations: &[Generation],
    current: Option<u32>,
) -> Result<Vec<u32>> {
    let all: Vec<u32> = generations.iter().map(|g| g.number).collect();
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

    if let Some(c) = current {
        if to_delete.contains(&c) {
            anyhow::bail!(
                "Refusing to delete the active generation ({}). \
                 Switch to another generation first (cheni rollback).",
                c
            );
        }
    }
    Ok(to_delete)
}

/// Print the list of targets and ask for confirmation. Returns `false`
/// when the user aborts (or `true` immediately when `yes` is set).
fn confirm_targets(to_delete: &[u32], yes: bool) -> Result<bool> {
    println!(
        "Will delete {} generation(s):",
        to_delete.len().to_string().bold()
    );
    for n in to_delete {
        println!("  {} {}", "-".red(), n.to_string().bold());
    }
    println!();

    if yes {
        return Ok(true);
    }

    let confirm = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Proceed?")
        .default(false)
        .interact()?;
    if !confirm {
        println!("{}", "Aborted.".dimmed());
    }
    Ok(confirm)
}

/// Shell out to `sudo nix-env --delete-generations N M …`.
fn apply_deletion(to_delete: &[u32]) -> Result<()> {
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
        .map_err(|e| crate::nix::tools::tool_error("sudo", e))
        .context("running nix-env --delete-generations")?;
    if !status.success() {
        anyhow::bail!("Generation deletion failed");
    }
    println!(
        "\n{} {} generation(s) removed.",
        "✓".green(),
        to_delete.len()
    );
    Ok(())
}

/// Optional `--gc` follow-up.
fn run_gc() -> Result<()> {
    println!("\n{}", "Running garbage collection...".bold());
    let gc_status = Command::new("sudo")
        .args(["/run/current-system/sw/bin/nix-collect-garbage"])
        .status()
        .map_err(|e| crate::nix::tools::tool_error("sudo", e))
        .context("running nix-collect-garbage")?;
    if !gc_status.success() {
        anyhow::bail!("Garbage collection failed");
    }
    println!("\n{} Disk space reclaimed.", "✓".green());
    Ok(())
}

/// Parse a target spec string into a list of generation numbers.
/// Accepts "405", "405..410" (inclusive range).
fn parse_target_spec(spec: &str, all: &[u32]) -> Result<Vec<u32>> {
    let spec = spec.trim();
    if spec.is_empty() {
        anyhow::bail!("Empty generation spec — expected a number or a 'N..M' range");
    }

    if let Some((from, to)) = spec.split_once("..") {
        if from.is_empty() || to.is_empty() {
            anyhow::bail!(
                "Range '{}' is missing one bound — expected 'N..M' with both ends present",
                spec
            );
        }
        let from: u32 = from
            .parse()
            .with_context(|| format!("Invalid range start in '{}'", spec))?;
        let to: u32 = to
            .parse()
            .with_context(|| format!("Invalid range end in '{}'", spec))?;
        let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
        let matched: Vec<u32> = all.iter().copied().filter(|n| *n >= lo && *n <= hi).collect();
        if matched.is_empty() {
            anyhow::bail!(
                "Range {}..{} matches no existing generation",
                lo, hi
            );
        }
        Ok(matched)
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
///
/// Rejects `0d` / `0w` / etc — passing zero would mean "everything
/// older than right now", which is essentially "all generations".
/// That's never what the user wants and would silently nuke the
/// rollback history.
fn parse_duration_days(spec: &str) -> Result<u64> {
    let spec = spec.trim();
    if spec.is_empty() {
        anyhow::bail!("Empty duration — expected something like '30d', '2w', '6m', '1y'");
    }
    let (num_part, unit) = spec.split_at(
        spec.find(|c: char| !c.is_ascii_digit())
            .unwrap_or(spec.len()),
    );
    let n: u64 = num_part
        .parse()
        .with_context(|| format!("Expected a number, got '{}'", num_part))?;
    if n == 0 {
        anyhow::bail!(
            "Refusing zero duration ('{}') — that would match every generation. \
             Use '--keep N' if you want to drop all-but-N.",
            spec
        );
    }
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
    let current_num = current_generation_number(profiles_dir);

    let entries = std::fs::read_dir(profiles_dir)
        .context("Cannot read /nix/var/nix/profiles")?;

    let mut generations: Vec<Generation> = entries
        .flatten()
        .filter_map(|entry| build_generation(&entry, current_num))
        .collect();

    generations.sort_by_key(|g| g.number);
    debug!("Found {} generations", generations.len());
    Ok(generations)
}

/// Resolve `/nix/var/nix/profiles/system` → "system-407-link" → 407.
fn current_generation_number(profiles_dir: &std::path::Path) -> Option<u32> {
    let target = std::fs::read_link(profiles_dir.join("system")).ok()?;
    let name = target.file_name()?.to_str()?;
    parse_generation_number(name)
}

/// "system-407-link" → Some(407); anything else → None.
fn parse_generation_number(filename: &str) -> Option<u32> {
    filename
        .strip_prefix("system-")?
        .strip_suffix("-link")?
        .parse::<u32>()
        .ok()
}

/// Turn a single `system-N-link` directory entry into a Generation.
/// Returns None for entries that don't match the expected shape — keeps
/// the caller's iterator a clean filter_map chain.
fn build_generation(
    entry: &std::fs::DirEntry,
    current_num: Option<u32>,
) -> Option<Generation> {
    let name = entry.file_name();
    let number = parse_generation_number(name.to_str()?)?;
    let metadata = entry.metadata().ok()?;
    let date = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| format_unix_date(d.as_secs()))
        .unwrap_or_else(|| "?".to_string());
    let store_path = entry.path().to_string_lossy().to_string();
    let nixos_label = read_nixos_label(&entry.path());
    Some(Generation {
        number,
        date,
        is_current: current_num == Some(number),
        store_path,
        nixos_label,
    })
}

/// Pull the NixOS version label out of a generation symlink target.
/// `/nix/store/abc-nixos-system-morthinkpad-26.05.20260414.4bd9165`
/// → `Some("26.05.20260414.4bd9165")`.
fn read_nixos_label(symlink: &std::path::Path) -> Option<String> {
    let target = std::fs::read_link(symlink).ok()?;
    let target_str = target.to_string_lossy().to_string();
    let last = target_str.rsplit('/').next()?;
    let (_, rest) = last.split_once("nixos-system-")?;
    // rest = "morthinkpad-26.05.20260414.4bd9165"
    let (_, version) = rest.split_once('-')?;
    Some(version.to_string())
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
    let categories = classify_diff_lines(stdout);
    format_summary(&categories)
}

/// Tallied categorisation of a `nix store diff-closures` output — one
/// bucket per kind of change plus the running size delta in KiB.
#[derive(Default)]
struct DiffCategories {
    updated: Vec<(String, String)>,
    added: Vec<String>,
    removed: Vec<String>,
    rebuilt: Vec<String>,
    size_delta_kib: f64,
}

/// Walk the raw diff output and drop each non-empty, ANSI-stripped line
/// into the right bucket. The four rule patterns come from the actual
/// nix output format:
///   `foo: ∅ → 1.0`       → added
///   `foo: 1.0 → ∅`       → removed
///   `foo: 1.0 → 2.0`     → updated (version text kept)
///   `foo: 38.6 KiB`      → rebuilt (same version, closure changed)
/// Size-delta lines are parsed independently and summed — they can
/// appear alongside any of the above.
fn classify_diff_lines(stdout: &str) -> DiffCategories {
    let mut c = DiffCategories::default();
    for line in stdout.lines() {
        let clean = strip_ansi(line);
        let trimmed = clean.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(delta) = parse_size_delta(trimmed) {
            c.size_delta_kib += delta;
        }
        let Some((name, rest)) = trimmed.split_once(": ") else {
            continue;
        };
        let name = name.trim().to_string();
        if rest.contains("∅ →") || rest.contains("∅ ->") {
            c.added.push(name);
        } else if rest.contains("→ ∅") || rest.contains("-> ∅") {
            c.removed.push(name);
        } else if rest.contains(" → ") || rest.contains(" -> ") {
            let versions = rest.split(',').next().unwrap_or(rest).trim().to_string();
            c.updated.push((name, versions));
        } else {
            c.rebuilt.push(name);
        }
    }
    c
}

/// Compose the human-readable summary line from the tallied categories.
/// Returns "(identical closures)" when nothing at all changed, otherwise
/// a comma-joined list of category fragments. Size delta is appended
/// last and only when it exceeds a 0.1 KiB rounding threshold.
fn format_summary(c: &DiffCategories) -> Option<String> {
    if c.updated.is_empty() && c.added.is_empty() && c.removed.is_empty() && c.rebuilt.is_empty() {
        return Some("(identical closures)".to_string());
    }
    let mut parts = Vec::new();
    if !c.updated.is_empty() {
        parts.push(format_update_list(&c.updated));
    }
    if !c.added.is_empty() {
        parts.push(format!("+ {}", format_name_list(&c.added)));
    }
    if !c.removed.is_empty() {
        parts.push(format!("- {}", format_name_list(&c.removed)));
    }
    if !c.rebuilt.is_empty() {
        parts.push(format!("⟳ {}", format_name_list(&c.rebuilt)));
    }
    if c.size_delta_kib.abs() >= 0.1 {
        parts.push(format_size_delta(c.size_delta_kib));
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
#[path = "tests/history.rs"]
mod diff_parser_tests;

#[cfg(test)]
#[path = "tests/history_specs.rs"]
mod spec_parser_tests;
