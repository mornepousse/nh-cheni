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

#[cfg(test)]
#[path = "tests/snapshot.rs"]
mod tests;
