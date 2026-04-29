//! `cheni snapshot` and `cheni restore` — port pins+freezes across machines.
//!
//! See `docs/superpowers/specs/2026-04-28-cheni-snapshot-design.md`.

#![allow(dead_code, unused_imports)]

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

/// Return the current UTC time formatted as RFC 3339 without pulling in chrono.
///
/// Format: `YYYY-MM-DDTHH:MM:SSZ`. Seconds precision is enough for a
/// human-readable timestamp in a snapshot file.
fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Manual decomposition — no external crate needed for a simple
    // ISO 8601 "T Z" timestamp.
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let mut days = secs / 86400; // days since 1970-01-01
    let mut year = 1970u32;
    loop {
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days_in_year = if leap { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days: [u64; 12] = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u32;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }
    let day = days + 1;
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Build a Snapshot from current pins + freezes + hostname.
pub(crate) fn compose_snapshot(
    pins: Vec<String>,
    freezes: freezes::Freezes,
    hostname: &str,
) -> Snapshot {
    Snapshot {
        format_version: FORMAT_VERSION,
        created_at: now_rfc3339(),
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

#[cfg(test)]
#[path = "tests/snapshot.rs"]
mod tests;
