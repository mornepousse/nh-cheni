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

    println!("  {} package(s) frozen", current.len().to_string().bold());
    println!();

    let total = current.len();
    for (idx, (name, entry)) in current.iter().enumerate() {
        let glyph = crate::util::tree_glyph(idx, total);
        println!(
            "  {} {:<28} {} {}",
            glyph.dimmed(),
            name.bold(),
            entry.version.dimmed(),
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
pub fn freeze_one(name: &str) -> Result<()> {
    let nix_config = config::detect()?;

    if !config::is_initialized(&nix_config.flake_dir) {
        super::check::print_first_run_hint();
        return Ok(());
    }

    reject_if_pinned(&nix_config.flake_dir, name)?;
    let store_pkg = store::find_by_name(name)?;
    let installed_version = store_pkg.version.clone();

    let existing = freezes::read(&nix_config.flake_dir)?
        .get(name)
        .cloned();
    print_freeze_header(name, &installed_version, existing.as_ref());

    println!();
    println!("  {}", "Reading current nixpkgs revision from flake.lock…".dimmed());
    let rev = flake::read_nixpkgs_rev(&nix_config.flake_dir)?;
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
    print_freeze_contract(name, &installed_version);
    if !confirm(&format!("Freeze {} at {}?", name, installed_version), true)? {
        println!("{}", "  Cancelled — nothing frozen.".yellow());
        return Ok(());
    }

    let entry = freezes::FreezeEntry {
        rev,
        nar_hash,
        version: installed_version.clone(),
        frozen_at: today_iso(),
    };
    let newly_frozen = freezes::add(&nix_config.flake_dir, name, entry)?;
    if newly_frozen {
        println!(
            "\n{} Froze {} at {}.",
            "✓".green(),
            name.bold(),
            installed_version.dimmed()
        );
    } else {
        println!(
            "\n{} Updated freeze for {} — now held at {}.",
            "✓".green(),
            name.bold(),
            installed_version.dimmed()
        );
    }
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
fn print_freeze_contract(name: &str, installed: &str) {
    println!("  {}", "What this does:".bold());
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

#[cfg(test)]
#[path = "tests/freeze.rs"]
mod tests;
