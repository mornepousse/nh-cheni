//! `cheni snapshot` and `cheni restore` — port pins+freezes across machines.
//!
//! See `docs/superpowers/specs/2026-04-28-cheni-snapshot-design.md`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm};
use serde::{Deserialize, Serialize};

use crate::nix::{config, freezes, pins};

/// Schema version for the snapshot file. Increment if the on-disk format
/// gains breaking changes; `restore` bails when reading a higher version.
pub(crate) const FORMAT_VERSION: u32 = 1;

/// Serialised form of pins + freezes plus enough metadata to be
/// portable across machines.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub format_version: u32,
    pub created_at: String,
    pub hostname: String,
    pub pins: Vec<String>,
    pub freezes: freezes::Freezes,
}

/// Diff produced by `compute_diff`: pins/freezes added, removed, or changed.
#[derive(Debug, Default)]
pub(crate) struct RestoreDiff {
    pub pins_added: Vec<String>,
    pub pins_removed: Vec<String>,
    pub freezes_added: Vec<String>,
    pub freezes_removed: Vec<String>,
    pub freezes_changed: Vec<String>,
}

impl RestoreDiff {
    pub fn is_empty(&self) -> bool {
        self.pins_added.is_empty()
            && self.pins_removed.is_empty()
            && self.freezes_added.is_empty()
            && self.freezes_removed.is_empty()
            && self.freezes_changed.is_empty()
    }
}

/// Build a Snapshot from current pins + freezes + hostname.
pub(crate) fn compose_snapshot(
    pins: Vec<String>,
    freezes: freezes::Freezes,
    hostname: &str,
) -> Snapshot {
    Snapshot {
        format_version: FORMAT_VERSION,
        created_at: crate::nix::timeline::now_rfc3339(),
        hostname: hostname.to_string(),
        pins,
        freezes,
    }
}

/// Compute the diff between current state and a snapshot to be restored.
pub(crate) fn compute_diff(
    current_pins: &[String],
    current_freezes: &freezes::Freezes,
    snapshot: &Snapshot,
) -> RestoreDiff {
    use std::collections::HashSet;

    let current_pin_set: HashSet<&String> = current_pins.iter().collect();
    let snapshot_pin_set: HashSet<&String> = snapshot.pins.iter().collect();

    let mut pins_added: Vec<String> = snapshot_pin_set
        .difference(&current_pin_set)
        .map(|s| (*s).clone())
        .collect();
    let mut pins_removed: Vec<String> = current_pin_set
        .difference(&snapshot_pin_set)
        .map(|s| (*s).clone())
        .collect();
    pins_added.sort();
    pins_removed.sort();

    let mut freezes_added = Vec::new();
    let mut freezes_removed = Vec::new();
    let mut freezes_changed = Vec::new();

    for (name, entry) in &snapshot.freezes {
        match current_freezes.get(name) {
            None => freezes_added.push(name.clone()),
            Some(current)
                if current.version != entry.version || current.rev != entry.rev =>
            {
                freezes_changed.push(name.clone());
            }
            _ => {} // identical
        }
    }
    for name in current_freezes.keys() {
        if !snapshot.freezes.contains_key(name) {
            freezes_removed.push(name.clone());
        }
    }
    freezes_added.sort();
    freezes_removed.sort();
    freezes_changed.sort();

    RestoreDiff {
        pins_added,
        pins_removed,
        freezes_added,
        freezes_removed,
        freezes_changed,
    }
}

/// Run `cheni snapshot`. Reads pins + freezes, composes a Snapshot,
/// writes JSON to `out` (or stdout if None).
pub fn snapshot(out: Option<PathBuf>) -> Result<()> {
    let nix_config = config::detect()?;
    let pins = pins::read(&nix_config.flake_dir)?;
    let freezes = freezes::read(&nix_config.flake_dir)?;
    let snap = compose_snapshot(pins, freezes, &nix_config.hostname);
    let json = serde_json::to_string_pretty(&snap)?;

    let n_pins = snap.pins.len();
    let n_freezes = snap.freezes.len();

    match out {
        Some(path) => {
            crate::util::atomic_write(&path, &json)?;
            eprintln!(
                "{} Snapshot written to {} ({} pin(s), {} freeze(s)).",
                "✓".green(),
                path.display(),
                n_pins,
                n_freezes
            );
        }
        None => {
            println!("{}", json);
            eprintln!(
                "{} Snapshotted {} pin(s) + {} freeze(s) to stdout.",
                "✓".green(),
                n_pins,
                n_freezes
            );
        }
    }
    Ok(())
}

/// Run `cheni restore <FILE>`. Replaces local pins+freezes with the
/// content of the snapshot file, after showing a diff and asking
/// confirmation (default-no since this is destructive).
pub fn restore(file: &Path, yes: bool) -> Result<()> {
    let raw = std::fs::read_to_string(file)
        .with_context(|| format!("reading snapshot file {}", file.display()))?;
    let snap: Snapshot = serde_json::from_str(&raw)
        .with_context(|| format!("parsing snapshot file {}", file.display()))?;

    if snap.format_version > FORMAT_VERSION {
        bail!(
            "snapshot uses format version {}, this cheni supports up to {}. Update cheni.",
            snap.format_version,
            FORMAT_VERSION
        );
    }

    let nix_config = config::detect()?;
    let current_pins = pins::read(&nix_config.flake_dir)?;
    let current_freezes = freezes::read(&nix_config.flake_dir)?;

    let diff = compute_diff(&current_pins, &current_freezes, &snap);
    if diff.is_empty() {
        println!("{} Already in sync with snapshot.", "✓".green());
        return Ok(());
    }

    println!("{}", "=== cheni restore ===".bold());
    println!();
    println!(
        "Snapshot from {} (created {}).",
        snap.hostname.dimmed(),
        snap.created_at.dimmed()
    );
    println!();
    print_diff_section("Pins to add", &diff.pins_added, "+", colored::Color::Green);
    print_diff_section("Pins to remove", &diff.pins_removed, "-", colored::Color::Red);
    print_diff_section(
        "Freezes to add",
        &diff.freezes_added,
        "+",
        colored::Color::Green,
    );
    print_diff_section(
        "Freezes to remove",
        &diff.freezes_removed,
        "-",
        colored::Color::Red,
    );
    print_diff_section(
        "Freezes to change",
        &diff.freezes_changed,
        "~",
        colored::Color::Yellow,
    );

    if !yes {
        let theme = ColorfulTheme::default();
        let go = Confirm::with_theme(&theme)
            .with_prompt("Apply this restore? Local pins+freezes will be REPLACED.")
            .default(false)
            .interact()
            .map_err(|e| anyhow::anyhow!("reading confirmation: {e}"))?;
        if !go {
            println!("{}", "  Cancelled — nothing changed.".yellow());
            return Ok(());
        }
    }

    apply_restore(&nix_config.flake_dir, &snap)?;
    crate::nix::timeline::record(
        "restore",
        None,
        serde_json::json!({
            "from": snap.hostname,
            "n_pins": snap.pins.len(),
            "n_freezes": snap.freezes.len(),
        }),
    );
    println!(
        "{} Restored {} pin(s) + {} freeze(s) from snapshot.",
        "✓".green(),
        snap.pins.len(),
        snap.freezes.len()
    );
    Ok(())
}

fn print_diff_section(label: &str, items: &[String], glyph: &str, color: colored::Color) {
    if items.is_empty() {
        return;
    }
    println!("{}:", label.bold());
    for name in items {
        println!("  {} {}", glyph.color(color), name);
    }
    println!();
}

/// Replace local pins and freezes with the snapshot's content.
fn apply_restore(flake_dir: &Path, snap: &Snapshot) -> Result<()> {
    pins::clear(flake_dir)?;
    if !snap.pins.is_empty() {
        pins::add(flake_dir, &snap.pins)?;
    }
    freezes::clear(flake_dir)?;
    for (name, entry) in &snap.freezes {
        freezes::add(flake_dir, name, entry.clone())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/snapshot.rs"]
mod tests;
