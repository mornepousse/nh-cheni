//! `cheni freeze` command.
//!
//! Holds a package at its current nixpkgs revision while the rest of
//! the system continues to move — the **opposite** of `cheni pin`,
//! which routes a package through `nixpkgs-latest` to get a *newer*
//! version.
//!
//! The freeze is driven by `package-freezes.json` at the flake root:
//! each entry stores the nixpkgs rev + narHash the package is held
//! at, and the overlay (see `cmd::init`) routes the package through
//! `builtins.fetchTree` at that rev.

use anyhow::{Context, Result};
use colored::Colorize;

use crate::nix::{config, flake, freezes, pins, store};

/// Run `cheni freeze` with no arguments — list currently frozen packages.
pub fn list_freezes() -> Result<()> {
    let nix_config = config::detect()?;
    let current = freezes::read(&nix_config.flake_dir)?;

    println!("{}\n", "=== cheni freeze (list) ===".bold());

    if current.is_empty() {
        println!("  {}", "no packages frozen.".dimmed());
        println!();
        println!(
            "  Freeze a package at its current version with '{}'.",
            "cheni freeze <name>".bold()
        );
        return Ok(());
    }

    println!(
        "  {} {} frozen",
        current.len().to_string().bold(),
        crate::util::pluralize(current.len(), "package")
    );
    println!();

    let total = current.len();
    for (idx, (name, entry)) in current.iter().enumerate() {
        let glyph = crate::util::tree_glyph(idx, total);
        let mode = match entry.major_constraint {
            Some(n) => format!(" [major {}]", n).cyan().to_string(),
            None => String::new(),
        };
        println!(
            "  {} {:<28} {}{} {}",
            glyph.dimmed(),
            name.bold(),
            entry.version.dimmed(),
            mode,
            format!("(since {}, rev {})", entry.frozen_at, flake::short_hash(&entry.rev)).dimmed()
        );
    }

    println!();
    println!(
        "  {} Release one with '{}', or all at once with '{}'.",
        "·".dimmed(),
        "cheni unfreeze <name>".bold(),
        "cheni unfreeze --all".bold()
    );
    Ok(())
}

/// Run `cheni freeze <package>`.
///
/// Freezes the named package at the current `nixpkgs` rev. Aborts
/// cleanly (with a useful message) when the user hasn't run `cheni init`,
/// when the package isn't installed, when it's already pinned (the two
/// mechanisms are mutually exclusive), or when the user cancels at the
/// preview prompt.
pub fn freeze_one(name: &str, major_constraint: Option<u32>) -> Result<()> {
    let nix_config = config::detect()?;
    if !config::is_initialized(&nix_config.flake_dir) {
        super::check::print_first_run_hint();
        return Ok(());
    }

    reject_if_pinned(&nix_config.flake_dir, name)?;
    let installed_version = store::find_by_name(name)?.version;

    // When `--major N` is passed, sanity-check that the installed
    // version actually starts with that major — otherwise the user is
    // about to freeze a 9.x package at "major 10" which is almost
    // certainly a typo.
    if let Some(n) = major_constraint {
        let parsed = crate::version::parse::parse_version(&installed_version);
        match parsed.first() {
            Some(&installed_major) if installed_major as u32 == n => { /* ok */ }
            Some(&installed_major) => anyhow::bail!(
                "--major {} doesn't match the installed version ({}, major {}).\n\
                 Did you mean --major {}?",
                n,
                installed_version,
                installed_major,
                installed_major
            ),
            None => anyhow::bail!(
                "Cannot parse a major version out of '{}' — drop --major or \
                 pick a different package.",
                installed_version
            ),
        }
    }

    let ctx = gather_freeze_context(&nix_config.flake_dir, name, &installed_version)?;

    print_freeze_contract(name, &installed_version, major_constraint);
    let prompt = match major_constraint {
        Some(n) => format!("Freeze {} at major {} (currently {})?", name, n, installed_version),
        None => format!("Freeze {} at {}?", name, installed_version),
    };
    if !confirm(&prompt, true)? {
        println!("{}", "  Cancelled — nothing frozen.".yellow());
        return Ok(());
    }

    apply_freeze(&nix_config.flake_dir, name, ctx, &installed_version, major_constraint)
}

/// Everything we need to build a `FreezeEntry`, plus the side effect
/// of printing the "preview" lines (header + rev + narHash) to the
/// user on the way.
///
/// Split out so `freeze_one` reads as a four-step orchestrator:
/// preflight → gather → confirm → apply.
struct FreezeContext {
    rev: String,
    nar_hash: String,
}

/// Step 2 of `freeze_one`: print the header, read the current nixpkgs
/// rev, prefetch the narHash. Returns the rev+narHash ready for
/// `apply_freeze`. Exits early (via `?`) if reading flake.lock or
/// prefetching fails — those are hard errors the caller should
/// surface as-is.
fn gather_freeze_context(
    flake_dir: &std::path::Path,
    name: &str,
    installed_version: &str,
) -> Result<FreezeContext> {
    let existing = freezes::read(flake_dir)?.get(name).cloned();
    print_freeze_header(name, installed_version, existing.as_ref());

    println!();
    println!("  {}", "Reading current nixpkgs revision from flake.lock…".dimmed());
    let rev = flake::read_nixpkgs_rev(flake_dir)?;
    println!(
        "  {} rev {}",
        "·".dimmed(),
        flake::short_hash(&rev).dimmed()
    );

    println!(
        "  {}",
        "Prefetching tarball for pure eval (nix flake prefetch)…".dimmed()
    );
    let nar_hash = flake::prefetch_nixpkgs_rev(&rev)
        .context("Could not prefetch the nixpkgs tarball — freeze aborted.")?;
    println!("  {} {}", "·".dimmed(), short_nar_hash(&nar_hash).dimmed());
    println!();

    Ok(FreezeContext { rev, nar_hash })
}

/// Step 4 of `freeze_one`: write the entry to `package-freezes.json`
/// and print a success line tailored to whether this was a new
/// freeze or a replacement. The `newly_frozen` bool comes back from
/// `freezes::add` — we don't have to pre-compute it.
fn apply_freeze(
    flake_dir: &std::path::Path,
    name: &str,
    ctx: FreezeContext,
    installed_version: &str,
    major_constraint: Option<u32>,
) -> Result<()> {
    let entry = freezes::FreezeEntry {
        rev: ctx.rev,
        nar_hash: ctx.nar_hash,
        version: installed_version.to_string(),
        frozen_at: today_iso(),
        major_constraint,
    };
    let newly_frozen = freezes::add(flake_dir, name, entry)?;

    let tail = match major_constraint {
        Some(n) => format!(" (tracking major {})", n),
        None => String::new(),
    };
    let summary = if newly_frozen {
        format!("Froze {} at {}{}.", name.bold(), installed_version.dimmed(), tail)
    } else {
        format!(
            "Updated freeze for {} — now held at {}{}.",
            name.bold(),
            installed_version.dimmed(),
            tail
        )
    };
    println!("\n{} {}", "✓".green(), summary);
    println!("Run '{}' to apply.", "cheni build".bold());
    Ok(())
}

/// Short-circuit with a helpful error when the user tries to freeze a
/// package that is already pinned through `nixpkgs-latest`. The two
/// mechanisms are mutually exclusive — they'd both register the same
/// attribute on the overlay and one would silently win.
fn reject_if_pinned(flake_dir: &std::path::Path, name: &str) -> Result<()> {
    let current_pins = pins::read(flake_dir)?;
    if current_pins.iter().any(|p| p == name) {
        anyhow::bail!(
            "'{name}' is currently pinned to nixpkgs-latest.\n\n\
             Pin and freeze are opposite operations (pin = newer via nixpkgs-latest,\n\
             freeze = held at current rev). Run '{}' first, then '{}'.",
            format!("cheni unpin {name}").bold(),
            format!("cheni freeze {name}").bold()
        );
    }
    Ok(())
}

// `find_in_store` was removed — call `store::find_by_name` directly.

/// Header block shown before the preview. When replacing an existing
/// freeze, call out what's changing so the user doesn't silently lose
/// the old hold.
fn print_freeze_header(name: &str, installed: &str, existing: Option<&freezes::FreezeEntry>) {
    println!("{}\n", "=== cheni freeze ===".bold());
    match existing {
        None => {
            println!(
                "  Freezing {} at the current store version {}.",
                name.bold(),
                installed.dimmed()
            );
        }
        Some(prev) => {
            println!(
                "  {} is already frozen at {} (since {}).",
                name.bold(),
                prev.version.dimmed(),
                prev.frozen_at.dimmed()
            );
            println!(
                "  Re-freezing will replace the existing hold with {} (today's store version).",
                installed.dimmed()
            );
        }
    }
}

/// Educational block before the confirm — mirror of `pin::print_pin_contract`
/// so the two commands feel like a matched pair. The copy is deliberately
/// sharp on the inverse semantic ("held" vs "tracks nixpkgs-latest").
///
/// With `major_constraint`, the wording pivots from "strict lock" to
/// "track the latest within major N".
fn print_freeze_contract(name: &str, installed: &str, major_constraint: Option<u32>) {
    println!("  {}", "What this does:".bold());
    match major_constraint {
        None => {
            println!(
                "    Holds {} at {} regardless of nixpkgs updates.",
                name.bold(),
                installed.dimmed()
            );
            println!(
                "    Next '{}' will keep {} at this version — other packages move as usual.",
                "cheni upgrade".bold(),
                name
            );
        }
        Some(n) => {
            println!(
                "    Tracks major {} of {} (currently {}).",
                n.to_string().bold(),
                name.bold(),
                installed.dimmed()
            );
            println!(
                "    Next '{}' will bump {} to the latest {}.x available in nixpkgs,",
                "cheni upgrade".bold(),
                name,
                n
            );
            println!(
                "    and hold it at the last {}.x once upstream moves to {}.",
                n,
                n + 1
            );
        }
    }
    println!(
        "    The freeze stays active until you run '{}'.",
        format!("cheni unfreeze {}", name).bold()
    );
    println!(
        "    This is the opposite of '{}' (which routes through nixpkgs-latest = newer).",
        "cheni pin".bold()
    );
    println!();
}

// `confirm` was removed — call `crate::util::confirm` directly.
use crate::util::confirm;

/// Compact `YYYY-MM-DD` stamp for the `frozen_at` field. Delegates
/// to `crate::util::format_ymd` for the arithmetic.
fn today_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    crate::util::format_ymd(secs)
}

// `short_rev` was folded into `crate::nix::flake::short_hash`.

/// Show the narHash as `sha256-AAAA…ZZZZ` so it fits on a line.
/// Pure display — full value is preserved on disk.
fn short_nar_hash(hash: &str) -> String {
    if hash.len() <= 24 {
        return hash.to_string();
    }
    let head: String = hash.chars().take(12).collect();
    let tail: String = hash
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}…{}", head, tail)
}

/// Outcome of refreshing one constrained freeze entry. Reported back
/// to the caller (typically `cheni upgrade`) so it can render a
/// single-line summary per entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Couldn't query upstream nixpkgs — entry unchanged.
    Unknown,
    /// Upstream still on the tracked major and same version as
    /// what's stored — nothing to do.
    UpToDate { version: String },
    /// Upstream still on the tracked major but a newer `X.y.z` is
    /// available; the entry was bumped to that version.
    Bumped { old_version: String, new_version: String },
    /// Upstream has moved past the tracked major — entry held at
    /// the previous rev, user should decide.
    Held {
        frozen_version: String,
        upstream_version: String,
        tracked_major: u32,
    },
}

/// Walk every constrained freeze and refresh the ones whose upstream
/// nixpkgs version still matches the `major_constraint`. Returns a
/// per-entry outcome so the caller can display what happened.
///
/// - No constrained freezes → returns an empty Vec, no IO beyond
///   reading `package-freezes.json`.
/// - Any bumps → writes the updated freezes file atomically.
/// - Unreachable nixpkgs (network, missing rev) → every constrained
///   entry reports `Unknown`, file is not modified.
pub fn refresh_constrained_freezes(
    flake_dir: &std::path::Path,
) -> Result<Vec<(String, RefreshOutcome)>> {
    let current = freezes::read(flake_dir)?;
    let constrained: Vec<(String, freezes::FreezeEntry)> = current
        .iter()
        .filter(|(_, e)| e.major_constraint.is_some())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if constrained.is_empty() {
        return Ok(Vec::new());
    }

    // Current nixpkgs state the user just flake-updated to.
    let current_rev = flake::read_nixpkgs_rev(flake_dir)?;
    // Prefetching is the expensive step — fail-open so an offline
    // refresh doesn't block the upgrade. We report Unknown for each
    // constrained entry instead.
    let current_nar = match flake::prefetch_nixpkgs_rev(&current_rev) {
        Ok(h) => h,
        Err(e) => {
            tracing::debug!("freeze refresh: prefetch failed ({}), reporting Unknown", e);
            return Ok(constrained
                .into_iter()
                .map(|(name, _)| (name, RefreshOutcome::Unknown))
                .collect());
        }
    };

    let mut outcomes = Vec::with_capacity(constrained.len());
    let mut updated = current.clone();
    for (name, entry) in constrained {
        let constraint = entry.major_constraint.expect("filtered to Some");
        let Some(upstream_version) =
            flake::query_pkg_version_at_rev(&current_rev, &current_nar, &name)
        else {
            outcomes.push((name, RefreshOutcome::Unknown));
            continue;
        };
        let parsed = crate::version::parse::parse_version(&upstream_version);
        let Some(&first) = parsed.first() else {
            outcomes.push((name, RefreshOutcome::Unknown));
            continue;
        };
        let upstream_major = first as u32;
        if upstream_major == constraint {
            if upstream_version == entry.version {
                outcomes.push((name, RefreshOutcome::UpToDate { version: upstream_version }));
            } else {
                let new_entry = freezes::FreezeEntry {
                    rev: current_rev.clone(),
                    nar_hash: current_nar.clone(),
                    version: upstream_version.clone(),
                    frozen_at: today_iso(),
                    major_constraint: Some(constraint),
                };
                updated.insert(name.clone(), new_entry);
                outcomes.push((
                    name,
                    RefreshOutcome::Bumped {
                        old_version: entry.version.clone(),
                        new_version: upstream_version,
                    },
                ));
            }
        } else {
            outcomes.push((
                name,
                RefreshOutcome::Held {
                    frozen_version: entry.version.clone(),
                    upstream_version,
                    tracked_major: constraint,
                },
            ));
        }
    }

    if updated != current {
        freezes::write(flake_dir, &updated)?;
    }
    Ok(outcomes)
}

/// Render the outcome table from `refresh_constrained_freezes` for
/// display in `cheni upgrade`. No-op when every entry is `UpToDate`
/// or the list is empty.
pub fn print_refresh_summary(outcomes: &[(String, RefreshOutcome)]) {
    let any_interesting = outcomes.iter().any(|(_, o)| {
        !matches!(o, RefreshOutcome::UpToDate { .. })
    });
    if !any_interesting {
        return;
    }
    println!();
    println!("  {}", "Freeze refresh:".bold());
    for (name, outcome) in outcomes {
        match outcome {
            RefreshOutcome::UpToDate { .. } => {}
            RefreshOutcome::Unknown => {
                println!(
                    "    {} {:<28} {}",
                    "?".dimmed(),
                    name.bold(),
                    "(upstream version unavailable)".dimmed()
                );
            }
            RefreshOutcome::Bumped { old_version, new_version } => {
                println!(
                    "    {} {:<28} {} → {}",
                    "↑".green(),
                    name.bold(),
                    old_version.dimmed(),
                    new_version.bold()
                );
            }
            RefreshOutcome::Held { frozen_version, upstream_version, tracked_major } => {
                println!(
                    "    {} {:<28} held at {} — upstream now {} (> major {})",
                    "⚠".yellow().bold(),
                    name.bold(),
                    frozen_version.dimmed(),
                    upstream_version.yellow(),
                    tracked_major
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/freeze.rs"]
mod tests;
