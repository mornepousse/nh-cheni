//! Step 1 of `cheni upgrade`: refresh flake inputs.
//!
//! Owns the live-progress streamer, the post-step summary parser,
//! the dirty-lock warning, and the anti-downgrade order check that
//! gates the `--pins-only` path.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use super::summary::UpgradeContext;

/// One input update parsed out of `nix flake update`'s chatty stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InputUpdate {
    pub(super) name: String,
    pub(super) old_date: String,
    pub(super) new_date: String,
}

/// Outcome of comparing `nixpkgs.lastModified` vs `nixpkgs-latest.lastModified`.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum InputOrder {
    /// nixpkgs-latest is at a newer commit (safe to apply pins).
    LatestIsNewer,
    /// Both at the same commit (pins would be no-ops).
    Same,
    /// nixpkgs-latest is older (would cause downgrades).
    LatestIsOlder,
    /// Couldn't determine (lock unreadable / inputs missing).
    Unknown,
}

/// Step 1: refresh flake inputs.
///
/// `pins_only = false` runs a plain `nix flake update`, which bumps
/// every input. `pins_only = true` narrows the scope to the
/// `nixpkgs-latest` input — that's the one the per-package overlay
/// reads to apply pins, so it's the only refresh worth doing when
/// the user just wants their pin policy to take effect.
///
/// Streams meaningful stderr events live (the per-input bullets and
/// any warnings/errors) so the user sees progress instead of staring
/// at the step header for the duration of a network fetch. The full
/// stderr is also captured for the clean post-step summary that
/// follows — `nix flake update` prints its narrative on stderr,
/// never on stdout.
pub(super) fn update_flake_inputs(flake_dir: &Path, pins_only: bool) -> Result<UpgradeContext> {
    use std::io::{BufRead, BufReader};

    let mut cmd = Command::new("nix");
    cmd.arg("flake").arg("update");
    if pins_only {
        cmd.arg("nixpkgs-latest");
    }
    let mut child = cmd
        .current_dir(flake_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;

    let stderr_pipe = child
        .stderr
        .take()
        .expect("stderr was set to piped, must be Some");
    let reader = BufReader::new(stderr_pipe);
    let mut captured = String::new();
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        stream_flake_update_progress(&line);
        captured.push_str(&line);
        captured.push('\n');
    }

    let status = child
        .wait()
        .context("waiting on nix flake update")?;
    if !status.success() {
        if !captured.is_empty() {
            eprintln!("{}", captured);
        }
        anyhow::bail!(
            "nix flake update failed. Common causes: \
             no network access, an input refers to a tag/branch that disappeared, \
             or a private repo without auth. Output above shows the specific error."
        );
    }

    let updates = parse_flake_update_events(&captured);
    print_flake_update_summary(&updates);
    Ok(UpgradeContext {
        inputs_updated: updates.len(),
        git_tree_dirty: detect_dirty_tree_warning(&captured),
    })
}

/// Warn the user when `flake.lock` already has uncommitted changes.
///
/// Shared between `cheni upgrade` (where it precedes the rebuild) and
/// `cheni preview` (where it shapes how to read the report — a dirty
/// lock means the preview reflects pending bumps from a prior run,
/// not just the latest fetch).
///
/// Without this surface, a previous `cheni upgrade` cancelled at the
/// confirmation prompt leaves the lock dirty — and the next rebuild
/// silently applies all those pre-existing input bumps on top of
/// whatever the new run fetches. That's how a `--pins-only` invocation
/// can end up rebuilding the kernel: it's not the pin scope, it's the
/// dirty lock that does the heavy lifting at rebuild time.
///
/// The wording is deliberately verbose: this is the kind of subtlety
/// that bites users only once, and only because nothing told them it
/// was happening.
pub(crate) fn warn_if_dirty_lock(flake_dir: &Path) {
    if !is_flake_lock_dirty(flake_dir) {
        return;
    }
    println!(
        "  {} {}",
        "⚠".yellow().bold(),
        "flake.lock has uncommitted input changes.".yellow()
    );
    println!(
        "    {}",
        "Likely from a previous upgrade that didn't reach the rebuild step.".dimmed()
    );
    println!(
        "    {}",
        "Any rebuild from now on will apply ALL of them — regardless of this run's scope.".dimmed()
    );
    println!(
        "    {}  {}    {}",
        "·".dimmed(),
        "git diff flake.lock".bold(),
        "to inspect".dimmed()
    );
    println!(
        "    {}  {}    {}",
        "·".dimmed(),
        "git checkout flake.lock".bold(),
        "to discard the pending bumps".dimmed()
    );
    println!();
}

/// Local alias to the shared `nix::git::is_flake_lock_dirty` so the
/// call sites in this module keep their narrow names.
fn is_flake_lock_dirty(flake_dir: &Path) -> bool {
    crate::nix::git::is_flake_lock_dirty(flake_dir)
}

/// Verify that `nixpkgs-latest` is strictly newer than `nixpkgs` at
/// the locked revisions. Returns `true` when the upgrade may proceed,
/// `false` after printing user-facing guidance for the two stop cases
/// (Same / LatestIsOlder). `Unknown` (unreadable lock) proceeds with a
/// debug warning rather than stranding the user.
///
/// Ported from the old `cheni update`; only relevant on the
/// `--pins-only` path. The full upgrade refreshes both inputs so the
/// ordering is irrelevant there.
pub(super) fn verify_nixpkgs_order(flake_dir: &Path) -> bool {
    match check_nixpkgs_order(flake_dir) {
        InputOrder::LatestIsNewer => {
            debug!("nixpkgs-latest is ahead of nixpkgs — safe to apply");
            println!("  {} nixpkgs-latest is ahead of nixpkgs.", "✓".green());
            true
        }
        InputOrder::Same => {
            println!(
                "  {} nixpkgs and nixpkgs-latest are at the same commit.",
                "⚠".yellow()
            );
            println!(
                "  Pins won't have any effect. Run '{}' for a full upgrade or '{}' to drop pins.",
                "cheni upgrade".bold(),
                "cheni unpin --all".bold(),
            );
            false
        }
        InputOrder::LatestIsOlder => {
            println!(
                "  {} nixpkgs-latest is BEHIND nixpkgs — skipping to prevent downgrades.",
                "✗".red()
            );
            println!(
                "  This can happen after a full '{}'. Pins are no longer needed — '{}'.",
                "cheni upgrade".bold(),
                "cheni unpin --all".bold(),
            );
            false
        }
        InputOrder::Unknown => {
            tracing::warn!("Could not compare nixpkgs revisions, proceeding anyway");
            println!(
                "  {} Could not compare revisions — proceeding anyway.",
                "·".dimmed()
            );
            true
        }
    }
}

pub(super) fn check_nixpkgs_order(flake_dir: &Path) -> InputOrder {
    let lock_path = flake_dir.join("flake.lock");
    let Ok(content) = std::fs::read_to_string(&lock_path) else {
        return InputOrder::Unknown;
    };
    let Ok(lock) = serde_json::from_str::<serde_json::Value>(&content) else {
        return InputOrder::Unknown;
    };
    let nixpkgs_time = get_input_timestamp(&lock, "nixpkgs");
    let latest_time = get_input_timestamp(&lock, "nixpkgs-latest");
    match (nixpkgs_time, latest_time) {
        (Some(base), Some(latest)) => {
            debug!(
                "nixpkgs lastModified: {}, nixpkgs-latest lastModified: {}",
                base, latest
            );
            if latest > base {
                InputOrder::LatestIsNewer
            } else if latest == base {
                InputOrder::Same
            } else {
                InputOrder::LatestIsOlder
            }
        }
        _ => InputOrder::Unknown,
    }
}

/// Read `<input>.lastModified` from a parsed flake.lock. Resolves via
/// the root node (root.inputs[name]) since the top-level node may be
/// a transitive entry rather than the root's direct input.
pub(super) fn get_input_timestamp(lock: &serde_json::Value, input_name: &str) -> Option<u64> {
    let root_input = lock
        .get("nodes")?
        .get("root")?
        .get("inputs")?
        .get(input_name)?;
    let node_name = match root_input.as_str() {
        Some(s) => s,
        None => input_name,
    };
    lock.get("nodes")?
        .get(node_name)?
        .get("locked")?
        .get("lastModified")?
        .as_u64()
}

/// Print the meaningful fragments of `nix flake update`'s stderr
/// as they arrive. The full output is mostly `Locked node …` chatter
/// that would drown the step header — we only surface:
///
/// - `• Updated input 'X':` bullets, condensed to a one-liner so the
///   user sees inputs landing in real time;
/// - `warning:` lines (typically "Git tree is dirty"), styled yellow
///   so they're hard to miss;
/// - `error:` lines styled red — they're rare here but if they come
///   the user should see them inline rather than only after the bail.
fn stream_flake_update_progress(line: &str) {
    let trimmed = line.trim_start();
    if let Some(name) = extract_updated_input_name(trimmed) {
        println!("    {} updated {}", "·".dimmed(), name.dimmed());
    } else if trimmed.starts_with("warning:") {
        println!("    {}", trimmed.yellow());
    } else if trimmed.starts_with("error:") {
        println!("    {}", trimmed.red());
    }
}

/// Nix prints `warning: Git tree '<path>' is dirty` (or `warning: dirty
/// Git tree '<path>'` on older nix) when the flake repo has
/// uncommitted changes. Detecting it lets the final summary explain
/// why a "no-op" upgrade still rebuilt artefacts.
pub(super) fn detect_dirty_tree_warning(stderr: &str) -> bool {
    stderr
        .lines()
        .any(|l| l.contains("Git tree") && l.contains("is dirty")
             || l.contains("dirty Git tree"))
}

/// Parse the `• Updated input 'X':` blocks out of `nix flake update`'s
/// stderr. Returns one `InputUpdate` per input that actually bumped.
///
/// The stanza is:
/// ```text
/// • Updated input 'NAME':
///     'url?…' (YYYY-MM-DD)
///   → 'url?…' (YYYY-MM-DD)
/// ```
pub(super) fn parse_flake_update_events(stderr: &str) -> Vec<InputUpdate> {
    let mut out = Vec::new();
    let lines: Vec<&str> = stderr.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(name) = extract_updated_input_name(line) {
            // Next two lines carry the old / new locator.
            let old_date = lines.get(i + 1).and_then(|l| extract_parenthesised_date(l));
            let new_date = lines.get(i + 2).and_then(|l| extract_parenthesised_date(l));
            if let (Some(old_date), Some(new_date)) = (old_date, new_date) {
                out.push(InputUpdate {
                    name,
                    old_date,
                    new_date,
                });
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// `• Updated input 'cheni':` → `Some("cheni")`.
fn extract_updated_input_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("• Updated input '")?;
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

/// Extract `YYYY-MM-DD` from a locator line like
/// `    'github:...?narHash=...' (2026-04-20)`.
fn extract_parenthesised_date(line: &str) -> Option<String> {
    let open = line.rfind('(')?;
    let close = line[open + 1..].find(')')?;
    let body = &line[open + 1..open + 1 + close];
    // Shape check: YYYY-MM-DD.
    if body.len() == 10
        && body.as_bytes()[4] == b'-'
        && body.as_bytes()[7] == b'-'
        && body.chars().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        })
    {
        Some(body.to_string())
    } else {
        None
    }
}

/// Render the flake-update outcome as a compact table. Silent when
/// nothing bumped (the separator + "already up to date" header is
/// enough).
fn print_flake_update_summary(updates: &[InputUpdate]) {
    if updates.is_empty() {
        println!("  {}", "Everything already up to date.".dimmed());
        return;
    }
    println!(
        "  {} {} {} updated:",
        "✓".green(),
        updates.len().to_string().bold(),
        crate::util::pluralize(updates.len(), "input")
    );
    for u in updates {
        println!(
            "    {:<20} {} → {}",
            u.name.bold(),
            u.old_date.dimmed(),
            u.new_date
        );
    }
}

#[cfg(test)]
#[path = "tests/flake_update.rs"]
mod tests;
