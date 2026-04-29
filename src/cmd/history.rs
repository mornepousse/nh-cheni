//! `cheni history` command.
//!
//! Lists all NixOS system generations with their date, kernel,
//! and the differences (added/changed/removed packages).
//!
//! Also handles selective generation deletion via the `--prune`,
//! `--delete`, `--keep`, and `--older-than` flags.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

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
    /// Print a one-line summary instead of the per-generation list.
    /// --diff overrides --brief (specific request wins).
    pub brief: bool,
}

/// A single NixOS generation.
///
/// Exposed at crate visibility so `cmd::rollback` can reuse the same
/// listing instead of parsing /nix/var/nix/profiles/ twice.
#[derive(Debug)]
pub(crate) struct Generation {
    /// Generation number.
    pub(crate) number: u32,
    /// Date the generation was created (human readable).
    pub(crate) date: String,
    /// Raw mtime of the generation symlink as Unix seconds, when
    /// available. Used by the pin/freeze annotation to time-travel
    /// `package-pins.json` and `package-freezes.json` via git log.
    /// `None` only on exotic filesystems that don't expose mtime.
    pub(crate) mtime_secs: Option<u64>,
    /// Whether this is the currently active generation.
    pub(crate) is_current: bool,
    /// Path to the generation in the store.
    pub(crate) store_path: String,
    /// NixOS version label (e.g. "26.05.20260414.4bd9165").
    pub(crate) nixos_label: Option<String>,
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

    // --diff overrides --brief (specific request wins).
    let brief = opts.brief && !opts.diff;

    if brief {
        return run_brief(&generations);
    }

    println!("{}\n", "=== cheni history ===".bold());

    let total = generations.len();
    let to_show = opts.limit.unwrap_or(10).min(total);

    // Show most recent first
    let displayed: Vec<&Generation> = generations.iter().rev().take(to_show).collect();

    // Resolve the flake dir once and gate the pin/freeze annotation on
    // it being a git repo. Failures here are silent — annotation is an
    // optional layer, the rest of `cheni history` works regardless.
    let pin_freeze_ctx = pin_freeze_annotation_dir();

    // Read timeline events once — used to annotate each gen with the
    // operations that happened in its window. Empty vec on any error.
    let timeline_events = crate::nix::timeline::read_events().unwrap_or_default();

    for (i, gen) in displayed.iter().enumerate() {
        print_generation_header(gen);
        // Print timeline events that fall in this gen's window.
        if let Some(this_mtime) = gen.mtime_secs {
            // "previous" in chronological terms = displayed[i + 1] (older)
            let prev_mtime = displayed.get(i + 1).and_then(|g| g.mtime_secs);
            let events = events_for_gen(&timeline_events, this_mtime, prev_mtime);
            print_gen_events(&events);
        }
        if i + 1 < displayed.len() {
            let previous = displayed[i + 1];
            print_generation_diff(previous, gen, opts.diff, opts.full);
            if let Some(flake_dir) = &pin_freeze_ctx {
                print_pin_freeze_delta(previous, gen, flake_dir);
            }
        }
    }

    println!();
    print_history_footer(total, to_show, opts.full);
    Ok(())
}

/// Print a one-line summary of the generation list.
/// Format: `<N> generations | latest: <date>`
/// Used by `--brief` mode (no spinner, no per-generation blocks).
pub(crate) fn run_brief(generations: &[Generation]) -> Result<()> {
    let total = generations.len();
    let latest = generations
        .iter()
        .max_by_key(|g| g.number)
        .map(|g| g.date.as_str())
        .unwrap_or("?");
    let current = generations.iter().find(|g| g.is_current).map(|g| g.number);
    let current_str = current
        .map(|n| format!(", current: gen {}", n))
        .unwrap_or_default();
    println!(
        "{} generation{} | latest: {}{}",
        total.to_string().bold(),
        if total == 1 { "" } else { "s" },
        latest.dimmed(),
        current_str.dimmed(),
    );
    Ok(())
}

/// Return the flake directory to use for pin/freeze history annotation,
/// or `None` if annotation should be skipped.
///
/// Skipped when:
/// - no flake can be detected (cheni was run outside a NixOS config),
/// - the flake dir isn't inside a git work tree (manual config without
///   versioning — we have no ground truth for "state at time T").
fn pin_freeze_annotation_dir() -> Option<PathBuf> {
    let dir = crate::nix::config::detect().ok()?.flake_dir;
    if !crate::nix::git::is_repo(&dir) {
        debug!(
            "skipping pin/freeze annotation: {} is not a git repo",
            dir.display()
        );
        return None;
    }
    Some(dir)
}

/// Render the pin/freeze delta line under a generation, when there is
/// any change since the previous generation.
///
/// Silent when:
/// - either generation has no usable mtime,
/// - the pin and freeze states are identical between the two timestamps.
fn print_pin_freeze_delta(previous: &Generation, current: &Generation, flake_dir: &Path) {
    let (Some(p_secs), Some(c_secs)) = (previous.mtime_secs, current.mtime_secs) else {
        return;
    };
    let prev_at = UNIX_EPOCH + Duration::from_secs(p_secs);
    let cur_at = UNIX_EPOCH + Duration::from_secs(c_secs);

    let prev_pins = crate::nix::pins::read_at_time(flake_dir, prev_at);
    let cur_pins = crate::nix::pins::read_at_time(flake_dir, cur_at);
    let prev_freezes = crate::nix::freezes::read_at_time(flake_dir, prev_at);
    let cur_freezes = crate::nix::freezes::read_at_time(flake_dir, cur_at);

    let Some(delta) = compute_pin_freeze_delta(
        &prev_pins,
        &cur_pins,
        &prev_freezes,
        &cur_freezes,
    ) else {
        return;
    };
    println!("      {}", format_pin_freeze_delta(&delta).dimmed());
}

/// Symmetric difference of two pin/freeze states. `None` when the
/// states are identical and there's nothing worth annotating.
///
/// Pure on the inputs so the formatting and the time-travel can be
/// tested independently — this lets the unit tests construct states
/// in memory without spinning up a fixture git repo.
pub(crate) fn compute_pin_freeze_delta(
    prev_pins: &[String],
    cur_pins: &[String],
    prev_freezes: &crate::nix::freezes::Freezes,
    cur_freezes: &crate::nix::freezes::Freezes,
) -> Option<PinFreezeDelta> {
    let prev_set: BTreeSet<&str> = prev_pins.iter().map(String::as_str).collect();
    let cur_set: BTreeSet<&str> = cur_pins.iter().map(String::as_str).collect();

    let pins_added: Vec<String> = cur_set
        .difference(&prev_set)
        .map(|s| (*s).to_string())
        .collect();
    let pins_removed: Vec<String> = prev_set
        .difference(&cur_set)
        .map(|s| (*s).to_string())
        .collect();

    let mut freezes_added: Vec<(String, String)> = Vec::new();
    let mut freezes_changed: Vec<(String, String, String)> = Vec::new();
    let mut freezes_removed: Vec<String> = Vec::new();

    for (name, entry) in cur_freezes {
        match prev_freezes.get(name) {
            None => freezes_added.push((name.clone(), entry.version.clone())),
            Some(prev) if prev.rev != entry.rev || prev.version != entry.version => {
                freezes_changed.push((name.clone(), prev.version.clone(), entry.version.clone()));
            }
            _ => {}
        }
    }
    for name in prev_freezes.keys() {
        if !cur_freezes.contains_key(name) {
            freezes_removed.push(name.clone());
        }
    }

    if pins_added.is_empty()
        && pins_removed.is_empty()
        && freezes_added.is_empty()
        && freezes_changed.is_empty()
        && freezes_removed.is_empty()
    {
        return None;
    }
    Some(PinFreezeDelta {
        pins_added,
        pins_removed,
        freezes_added,
        freezes_changed,
        freezes_removed,
    })
}

/// Tallied pin/freeze changes between two generations.
///
/// Frozen entries that bumped their `rev` while keeping the same name
/// land in `freezes_changed` (e.g. a `--major N` follow-up upgrade);
/// outright additions go to `freezes_added`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PinFreezeDelta {
    pub pins_added: Vec<String>,
    pub pins_removed: Vec<String>,
    pub freezes_added: Vec<(String, String)>,
    pub freezes_changed: Vec<(String, String, String)>,
    pub freezes_removed: Vec<String>,
}

/// Format the delta as a one-line annotation. Style mirrors the diff
/// summary line — same separators (`·`), same lower-case markers
/// (`+pinned …` / `-frozen …`) so the two annotation lines align
/// visually under each generation.
pub(crate) fn format_pin_freeze_delta(d: &PinFreezeDelta) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !d.pins_added.is_empty() {
        parts.push(format!("+pinned {}", join_with_overflow_owned(&d.pins_added, 3)));
    }
    if !d.pins_removed.is_empty() {
        parts.push(format!("-pinned {}", join_with_overflow_owned(&d.pins_removed, 3)));
    }
    if !d.freezes_added.is_empty() {
        let names: Vec<String> = d
            .freezes_added
            .iter()
            .map(|(n, v)| if v.is_empty() { n.clone() } else { format!("{}@{}", n, v) })
            .collect();
        parts.push(format!("+frozen {}", join_with_overflow_owned(&names, 3)));
    }
    if !d.freezes_changed.is_empty() {
        let names: Vec<String> = d
            .freezes_changed
            .iter()
            .map(|(n, ov, nv)| {
                if ov.is_empty() && nv.is_empty() {
                    n.clone()
                } else {
                    format!("{} {}→{}", n, ov, nv)
                }
            })
            .collect();
        parts.push(format!("~frozen {}", join_with_overflow_owned(&names, 3)));
    }
    if !d.freezes_removed.is_empty() {
        parts.push(format!(
            "-frozen {}",
            join_with_overflow_owned(&d.freezes_removed, 3)
        ));
    }
    parts.join(" · ")
}

/// Owned-string version of `join_with_overflow` used by the delta
/// formatter (the diff-summary helpers below take `&[&str]` since they
/// already have references on hand).
fn join_with_overflow_owned(items: &[String], max: usize) -> String {
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    join_with_overflow(&refs, max)
}

/// Format only the *loss* projection of a delta — pins and freezes
/// that existed in the "then" snapshot but no longer in "now".
///
/// Used by gen-deletion flows (`cheni history --prune/--delete/...`)
/// to flag generations whose pin/freeze state is no longer reproducible
/// from the current policy file. Returns `None` when nothing was lost
/// so the caller can stay silent in the common case.
///
/// `freezes_changed` is included on purpose: when a frozen entry's `rev`
/// has moved, the binary that was built under the *old* rev is unique
/// to the deleted generation — the current freeze rev produces a
/// different one.
pub(crate) fn format_pin_freeze_loss(d: &PinFreezeDelta) -> Option<String> {
    if d.pins_removed.is_empty()
        && d.freezes_removed.is_empty()
        && d.freezes_changed.is_empty()
    {
        return None;
    }
    let mut parts = Vec::new();
    if !d.pins_removed.is_empty() {
        parts.push(format!(
            "pinned {}",
            join_with_overflow_owned(&d.pins_removed, 3)
        ));
    }
    let mut frozen_names: Vec<String> = d.freezes_removed.clone();
    for (name, ov, _) in &d.freezes_changed {
        frozen_names.push(if ov.is_empty() {
            name.clone()
        } else {
            format!("{}@{}", name, ov)
        });
    }
    if !frozen_names.is_empty() {
        parts.push(format!(
            "frozen {}",
            join_with_overflow_owned(&frozen_names, 3)
        ));
    }
    Some(parts.join(", "))
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
    println!(
        "{}",
        "Tip: rollback with `cheni rollback <N>` or compare two with `cheni diff <from> <to>`."
            .dimmed()
    );
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
    if !confirm_targets(&to_delete, generations, opts.yes)? {
        return Ok(());
    }
    apply_deletion(&to_delete)?;
    if opts.gc {
        run_gc(opts.yes)?;
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

/// Print the list of targets, surface any pin/freeze policy loss the
/// deletion would entail, then ask for confirmation. Returns `false`
/// when the user aborts (or `true` immediately when `yes` is set).
fn confirm_targets(
    to_delete: &[u32],
    generations: &[Generation],
    yes: bool,
) -> Result<bool> {
    println!(
        "Will delete {} generation(s):",
        to_delete.len().to_string().bold()
    );
    for n in to_delete {
        println!("  {} {}", "-".red(), n.to_string().bold());
    }
    println!();

    if let Some(flake_dir) = pin_freeze_annotation_dir() {
        print_policy_loss_warning(to_delete, generations, &flake_dir);
    }

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

/// Flag the generations being deleted whose pin/freeze state at build
/// time is no longer in the current policy file.
///
/// Why this is worth surfacing: deleting such a generation drops the
/// only artefact that holds those exact binaries. `pinned X` is the
/// most painful case — pins route through `nixpkgs-latest`, so even
/// a `git checkout` of the old pins.json wouldn't reproduce the
/// version: nixpkgs-latest has moved on. `frozen X@vN` is partially
/// recoverable (you'd need both the old freeze rev and the binary's
/// build to be reproducible), but the warning still applies.
///
/// Silent in the common case (no policy drift across the deleted
/// gens) so routine pruning isn't cluttered.
fn print_policy_loss_warning(
    to_delete: &[u32],
    generations: &[Generation],
    flake_dir: &Path,
) {
    let cur_pins = crate::nix::pins::read(flake_dir).unwrap_or_default();
    let cur_freezes = crate::nix::freezes::read(flake_dir).unwrap_or_default();

    let mut affected: Vec<(u32, String)> = Vec::new();
    for n in to_delete {
        let Some(gen) = generations.iter().find(|g| g.number == *n) else {
            continue;
        };
        let Some(mt) = gen.mtime_secs else { continue };
        let at = UNIX_EPOCH + Duration::from_secs(mt);
        let then_pins = crate::nix::pins::read_at_time(flake_dir, at);
        let then_freezes = crate::nix::freezes::read_at_time(flake_dir, at);
        // Treat "then" as the older snapshot and "now" as the newer one
        // — the delta's `*_removed` and `freezes_changed` buckets then
        // contain exactly what's been lost since `then`.
        let Some(delta) = compute_pin_freeze_delta(
            &then_pins,
            &cur_pins,
            &then_freezes,
            &cur_freezes,
        ) else {
            continue;
        };
        if let Some(loss) = format_pin_freeze_loss(&delta) {
            affected.push((*n, loss));
        }
    }

    if affected.is_empty() {
        return;
    }

    println!(
        "  {}",
        format!(
            "Heads-up: {} of these had pins/freezes no longer in your config:",
            affected.len()
        )
        .yellow()
    );
    for (n, loss) in &affected {
        println!(
            "    gen {}  had {}",
            n.to_string().bold(),
            loss.dimmed()
        );
    }
    println!(
        "    {}",
        "Once removed, those exact binaries are gone — current policy alone can't reproduce them."
            .dimmed()
    );
    println!();
}

/// Shell out to `sudo nix-env --delete-generations N M …`.
pub(crate) fn apply_deletion(to_delete: &[u32]) -> Result<()> {
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
        anyhow::bail!(
            "nix-env --delete-generations failed. Generations may have already been removed — run `cheni history` to confirm current state."
        );
    }
    println!(
        "\n{} {} generation(s) removed.",
        "✓".green(),
        to_delete.len()
    );
    Ok(())
}

/// Optional `--gc` follow-up.
///
/// Runs a `--dry-run` first so the user sees how many store paths
/// would actually be removed before sudo-prompting for the real GC.
/// `yes` bypasses the confirmation (`cheni history ... --gc --yes`).
fn run_gc(yes: bool) -> Result<()> {
    println!("\n{}", "Running garbage collection...".bold());

    let preview = crate::nix::gc::preview(&[])?;
    if preview.paths == 0 {
        println!("  {} No dead store paths to remove.", "✓".green());
        return Ok(());
    }

    println!(
        "  {} store path(s) would be removed.",
        preview.paths.to_string().bold()
    );
    if !yes {
        let theme = ColorfulTheme::default();
        let go = Confirm::with_theme(&theme)
            .with_prompt("Proceed with garbage collection?")
            .default(false)
            .interact()
            .context("reading confirmation")?;
        if !go {
            println!("{}", "  Cancelled — store paths kept.".yellow());
            return Ok(());
        }
    }

    let gc_status = Command::new("sudo")
        .args(["/run/current-system/sw/bin/nix-collect-garbage"])
        .status()
        .map_err(|e| crate::nix::tools::tool_error("sudo", e))
        .context("running nix-collect-garbage")?;
    if !gc_status.success() {
        anyhow::bail!(
            "nix-collect-garbage failed. Disk may be full or a roots scan failed — try `nix-store --gc --print-roots` to inspect what's pinning paths."
        );
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
pub(crate) fn pick_oldest_beyond(all: &[u32], keep: usize) -> Vec<u32> {
    if all.len() <= keep {
        return Vec::new();
    }
    all[..all.len() - keep].to_vec()
}

/// Structured plan for "delete oldest N generations", with both
/// kept and deleted IDs surfaced so callers can render an audit.
#[derive(Debug, Clone)]
pub(crate) struct PrunePlan {
    /// Generation IDs that would be deleted (oldest beyond `keep`).
    pub deleted_ids: Vec<u32>,
    /// Generation IDs kept (the most recent `keep`, in ascending order).
    pub kept_ids: Vec<u32>,
}

impl PrunePlan {
    pub fn kept_count(&self) -> usize {
        self.kept_ids.len()
    }
}

/// Build a prune plan that keeps the `keep` most recent generations and
/// schedules the rest for deletion. Pure function — no I/O.
pub(crate) fn plan_prune_keep_n(generations: &[Generation], keep: usize) -> PrunePlan {
    let all_ids: Vec<u32> = generations.iter().map(|g| g.number).collect();
    let deleted_ids = pick_oldest_beyond(&all_ids, keep);
    let deleted_set: std::collections::HashSet<u32> = deleted_ids.iter().copied().collect();
    let kept_ids: Vec<u32> = all_ids.iter().copied().filter(|id| !deleted_set.contains(id)).collect();
    PrunePlan { deleted_ids, kept_ids }
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
pub(crate) fn read_generations() -> Result<Vec<Generation>> {
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
    let mtime_secs = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let date = mtime_secs
        .map(format_unix_date)
        .unwrap_or_else(|| "?".to_string());
    let store_path = entry.path().to_string_lossy().to_string();
    let nixos_label = read_nixos_label(&entry.path());
    Some(Generation {
        number,
        date,
        mtime_secs,
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

/// Format a unix timestamp as "YYYY-MM-DD HH:MM" (UTC). Thin wrapper
/// over `crate::util::format_ymd_hm` — kept as a named helper so the
/// intent-at-call-site reads clearly.
fn format_unix_date(secs: u64) -> String {
    crate::util::format_ymd_hm(secs)
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

/// Return events that fall within the time window `[prev_mtime, this_mtime + 60s]`.
///
/// The 60-second slop on the upper bound catches events that arrive slightly
/// after the gen activation (e.g. timeline::record called right after `nh os
/// switch` returns, which in turn finished just after the symlink was updated).
///
/// For the very first gen (no `prev_mtime`), the window opens 1 hour before
/// `this_mtime`.
pub(crate) fn events_for_gen(
    events: &[crate::nix::timeline::Event],
    this_mtime: u64,
    prev_mtime: Option<u64>,
) -> Vec<&crate::nix::timeline::Event> {
    let window_start = prev_mtime.unwrap_or_else(|| this_mtime.saturating_sub(3600));
    let window_end = this_mtime + 60;
    events
        .iter()
        .filter(|e| {
            let Some(t) = crate::nix::timeline::parse_rfc3339_to_unix(&e.ts) else {
                return false;
            };
            t >= window_start && t <= window_end
        })
        .collect()
}

/// Render the timeline events that belong to a generation as indented
/// sub-lines under the generation header.
fn print_gen_events(events: &[&crate::nix::timeline::Event]) {
    if events.is_empty() {
        return;
    }
    for e in events {
        let time = format_event_time(&e.ts);
        let pkg = e.package.as_deref().unwrap_or("");
        let summary = summarise_event_details(&e.kind, &e.details);
        let line = if summary.is_empty() {
            format!(
                "    {} {} {} {}",
                "·".dimmed(),
                time.dimmed(),
                e.kind.cyan(),
                pkg
            )
        } else {
            format!(
                "    {} {} {} {} {}",
                "·".dimmed(),
                time.dimmed(),
                e.kind.cyan(),
                pkg,
                summary.dimmed()
            )
        };
        println!("{}", line);
    }
}

/// Extract the HH:MM part from an RFC3339 timestamp.
/// "2026-04-28T11:30:00Z" → "11:30"
fn format_event_time(ts: &str) -> String {
    ts.split_once('T')
        .and_then(|(_, t)| t.split_once(':'))
        .map(|(h, rest)| {
            let m = rest.split(':').next().unwrap_or("00");
            format!("{h}:{m}")
        })
        .unwrap_or_else(|| ts.to_string())
}

/// Produce a short contextual suffix for an event's details field.
fn summarise_event_details(kind: &str, details: &serde_json::Value) -> String {
    if details.is_null() || details == &serde_json::json!({}) {
        return String::new();
    }
    match kind {
        "promote" | "demote" => {
            let from = details.get("from").and_then(|v| v.as_str()).unwrap_or("?");
            let to = details.get("to").and_then(|v| v.as_str()).unwrap_or("?");
            format!("({from} \u{2192} {to})")
        }
        "freeze" => details
            .get("version")
            .and_then(|v| v.as_str())
            .map(|v| format!("at {v}"))
            .unwrap_or_default(),
        "upgrade" | "build" => {
            let outcome = details.get("outcome").and_then(|v| v.as_str()).unwrap_or("?");
            let dur = details.get("duration_secs").and_then(|v| v.as_u64());
            match dur {
                Some(d) => format!("({outcome}, {d}s)"),
                None => format!("({outcome})"),
            }
        }
        "rollback" => {
            let to_gen = details.get("to_gen").and_then(|v| v.as_u64());
            match to_gen {
                Some(n) => format!("\u{2192} gen {n}"),
                None => String::new(),
            }
        }
        "restore" => {
            let host = details.get("from").and_then(|v| v.as_str()).unwrap_or("?");
            format!("from {host}")
        }
        _ => String::new(),
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
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;

    let stdout = String::from_utf8_lossy(&nix_diff.stdout);
    Ok(stdout.to_string())
}

#[cfg(test)]
#[path = "tests/history.rs"]
mod diff_parser_tests;

#[cfg(test)]
#[path = "tests/history_specs.rs"]
mod spec_parser_tests;

#[cfg(test)]
#[path = "tests/history_pin_delta.rs"]
mod pin_delta_tests;
