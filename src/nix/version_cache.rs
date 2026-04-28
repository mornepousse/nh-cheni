//! Nix-aware version cache keyed on `(input, rev, attr) → version`.
//!
//! Unlike the time-TTL Repology cache in `src/api/cache.rs`, this cache
//! invalidates naturally: a different `rev` produces a different key, so
//! updating a flake input automatically discards all stale entries for that
//! input without any wall-clock expiry logic.
//!
//! The file format is plain JSON, three levels deep:
//! ```json
//! {
//!   "nixpkgs": {
//!     "abc123def456": {
//!       "legacyPackages.x86_64-linux.firefox": "128.0"
//!     }
//!   }
//! }
//! ```
//!
//! Callers hold a `VersionCache`, call `lookup` / `store`, then `save` once
//! when done. There is no implicit auto-save: batching writes is the caller's
//! responsibility.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// On-disk version cache for `(input, rev, attr) → version` lookups.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct VersionCache {
    #[serde(flatten)]
    entries: HashMap<String, HashMap<String, HashMap<String, String>>>,
}

/// Delete the on-disk version cache (best effort — silently ignores
/// `NotFound`). Used by `cheni check --refresh`.
pub fn clear() -> std::io::Result<()> {
    let path = cache_path();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Return the canonical path to the on-disk version cache.
///
/// Follows the same convention as `src/api/cache.rs`:
/// `$XDG_CACHE_HOME/cheni/version-cache.json`
/// (falls back to `/tmp` when `dirs::cache_dir()` returns `None`).
pub fn cache_path() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("cheni")
        .join("version-cache.json")
}

impl VersionCache {
    /// Load the cache from `path`.
    ///
    /// - File missing → returns `Self::default()` (not an error).
    /// - File present but corrupt / unparseable → logs at `debug`, returns
    ///   `Self::default()` (treated as empty, NOT an error).
    /// - File present and valid → returns the deserialized cache.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            debug!("version_cache: no file at {}", path.display());
            return Ok(Self::default());
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                debug!("version_cache: failed to read {}: {}", path.display(), e);
                return Ok(Self::default());
            }
        };

        match serde_json::from_str::<Self>(&content) {
            Ok(cache) => {
                let count: usize = cache
                    .entries
                    .values()
                    .flat_map(|revs| revs.values())
                    .map(|attrs| attrs.len())
                    .sum();
                debug!(
                    "version_cache: loaded {} entries from {}",
                    count,
                    path.display()
                );
                Ok(cache)
            }
            Err(e) => {
                debug!(
                    "version_cache: failed to parse {}: {} — treating as empty",
                    path.display(),
                    e
                );
                Ok(Self::default())
            }
        }
    }

    /// Total number of cached `(input, rev, attr) → version` triples.
    /// Used for diagnostic reporting (`cheni bug-report`, `cheni doctor`).
    pub fn entry_count(&self) -> usize {
        self.entries
            .values()
            .flat_map(|revs| revs.values())
            .map(|attrs| attrs.len())
            .sum()
    }

    /// Look up a cached version for `(input, rev, attr)`.
    ///
    /// Returns `None` if any level of the hierarchy is absent.
    pub fn lookup(&self, input: &str, rev: &str, attr: &str) -> Option<String> {
        self.entries
            .get(input)?
            .get(rev)?
            .get(attr)
            .cloned()
    }

    /// Insert a `(input, rev, attr) → version` entry.
    ///
    /// Creates intermediate maps on demand. Does **not** persist to disk —
    /// call `save()` when the batch of stores is complete.
    pub fn store(&mut self, input: &str, rev: &str, attr: &str, version: &str) {
        self.entries
            .entry(input.to_string())
            .or_default()
            .entry(rev.to_string())
            .or_default()
            .insert(attr.to_string(), version.to_string());
    }

    /// Persist the cache to `path` atomically (tmp-file + rename).
    ///
    /// Creates the parent directory if it does not exist.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(&self)?;
        crate::util::atomic_write(path, &content)?;

        debug!("version_cache: saved to {}", path.display());
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/version_cache.rs"]
mod tests;
