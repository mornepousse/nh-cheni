//! Nix-aware version cache keyed on `(input, rev, attr) → version`.
//!
//! Foundation infrastructure for the smart UX layers planned in
//! later phases (obsolete-pin detection, repology cross-checks,
//! `check --pending`). Currently has no consumer in the fork — it's
//! shipped now so the wrapper-era cache file at
//! `$XDG_CACHE_HOME/cheni/version-cache.json` keeps being readable
//! and writable through the new code path, and so the next phase
//! that needs it doesn't have to design the file shape from scratch.
//!
//! The cache invalidates naturally: a different `rev` produces a
//! different key, so updating a flake input automatically discards
//! all stale entries for that input without any wall-clock expiry.
//!
//! Three-level JSON layout:
//!
//! ```json
//! {
//!   "nixpkgs": {
//!     "abc123def456…": {
//!       "legacyPackages.x86_64-linux.firefox": "128.0"
//!     }
//!   }
//! }
//! ```
//!
//! Callers hold a `VersionCache`, call `lookup` / `store`, then
//! `save` once when the batch is done. There is no implicit auto-save.

use std::{
  collections::HashMap,
  fs,
  io::Write,
  path::{Path, PathBuf},
};

use color_eyre::eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// On-disk version cache for `(input, rev, attr) → version` lookups.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct VersionCache {
  #[serde(flatten)]
  entries: HashMap<String, HashMap<String, HashMap<String, String>>>,
}

/// Canonical path to the on-disk version cache.
///
/// `$XDG_CACHE_HOME/cheni/version-cache.json`, falling back to
/// `$HOME/.cache/cheni/version-cache.json`, then `/tmp/cheni/...`.
/// Matches the wrapper-era location bit-for-bit so an existing cache
/// file is reused.
pub fn cache_path() -> PathBuf {
  cache_dir().join("version-cache.json")
}

fn cache_dir() -> PathBuf {
  if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
    return PathBuf::from(xdg).join("cheni");
  }
  if let Some(home) = std::env::var_os("HOME") {
    return PathBuf::from(home).join(".cache").join("cheni");
  }
  PathBuf::from("/tmp").join("cheni")
}

/// Best-effort delete of the cache file. Used by future
/// `--refresh`-style flags. Silently ignores `NotFound`.
///
/// # Errors
///
/// Returns the underlying `io::Error` for any failure other than the
/// file simply not existing.
pub fn clear() -> std::io::Result<()> {
  match fs::remove_file(cache_path()) {
    Ok(()) => Ok(()),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
    Err(e) => Err(e),
  }
}

impl VersionCache {
  /// Load the cache from `path`.
  ///
  /// - File missing → `Self::default()` (not an error).
  /// - File present but corrupt → DEBUG-log + `Self::default()`. The
  ///   cache is purely an optimisation; a corrupt file should never
  ///   block the call site.
  /// - File present and valid → deserialised cache.
  ///
  /// # Errors
  ///
  /// Returns an error only when the metadata check itself fails for
  /// reasons other than "missing file" — disk I/O, permissions, etc.
  pub fn load(path: &Path) -> Result<Self> {
    if !path.exists() {
      debug!("version_cache: no file at {}", path.display());
      return Ok(Self::default());
    }
    let content = match fs::read_to_string(path) {
      Ok(c) => c,
      Err(e) => {
        debug!(
          "version_cache: failed to read {}: {}",
          path.display(),
          e
        );
        return Ok(Self::default());
      },
    };
    match serde_json::from_str::<Self>(&content) {
      Ok(cache) => {
        debug!(
          "version_cache: loaded {} entries from {}",
          cache.entry_count(),
          path.display()
        );
        Ok(cache)
      },
      Err(e) => {
        debug!(
          "version_cache: failed to parse {}: {} — treating as empty",
          path.display(),
          e
        );
        Ok(Self::default())
      },
    }
  }

  /// Total number of cached `(input, rev, attr) → version` triples.
  pub fn entry_count(&self) -> usize {
    self
      .entries
      .values()
      .flat_map(|revs| revs.values())
      .map(HashMap::len)
      .sum()
  }

  /// Look up a cached version for `(input, rev, attr)`. Returns
  /// `None` if any level of the hierarchy is absent.
  pub fn lookup(
    &self,
    input: &str,
    rev: &str,
    attr: &str,
  ) -> Option<String> {
    self.entries.get(input)?.get(rev)?.get(attr).cloned()
  }

  /// Insert a `(input, rev, attr) → version` entry. Creates
  /// intermediate maps on demand. Does NOT persist — call
  /// [`save`](Self::save) when the batch is complete.
  pub fn store(
    &mut self,
    input: &str,
    rev: &str,
    attr: &str,
    version: &str,
  ) {
    self
      .entries
      .entry(input.to_string())
      .or_default()
      .entry(rev.to_string())
      .or_default()
      .insert(attr.to_string(), version.to_string());
  }

  /// Persist the cache to `path` atomically (tmp + rename, 0o600).
  /// Creates the parent directory if necessary.
  ///
  /// # Errors
  ///
  /// Returns an error when the parent directory can't be created or
  /// the atomic write fails.
  pub fn save(&self, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent)
        .with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(self)
      .context("serialising version cache to JSON")?;
    atomic_write(path, body.as_bytes())?;
    debug!("version_cache: saved to {}", path.display());
    Ok(())
  }
}

// Local atomic_write — third copy in this crate. Will be lifted to a
// shared cheni-util module on the next caller (phase 4+).
fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
  let parent = path.parent().unwrap_or_else(|| Path::new("."));
  let tmp_name = format!(
    "{}.tmp.{}",
    path.file_name().and_then(|n| n.to_str()).unwrap_or("nh-vc-tmp"),
    std::process::id()
  );
  let tmp = parent.join(tmp_name);
  {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
      use std::os::unix::fs::OpenOptionsExt;
      opts.mode(0o600);
    }
    let mut file = opts
      .open(&tmp)
      .with_context(|| format!("opening {} for write", tmp.display()))?;
    file
      .write_all(content)
      .with_context(|| format!("writing {}", tmp.display()))?;
    let _ = file.sync_all();
  }
  fs::rename(&tmp, path).with_context(|| {
    format!("renaming {} → {}", tmp.display(), path.display())
  })?;
  Ok(())
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;
  use tempfile::TempDir;

  #[test]
  fn load_returns_default_when_path_missing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("does-not-exist.json");
    let cache = VersionCache::load(&path).unwrap();
    assert_eq!(cache.entry_count(), 0);
  }

  #[test]
  fn load_returns_default_on_garbage_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("vc.json");
    fs::write(&path, b"{ not json").unwrap();
    let cache = VersionCache::load(&path).unwrap();
    assert_eq!(cache.entry_count(), 0);
  }

  #[test]
  fn store_then_lookup_roundtrips() {
    let mut cache = VersionCache::default();
    cache.store("nixpkgs", "abc", "pkgs.firefox", "128.0");
    cache.store("nixpkgs", "abc", "pkgs.mesa", "24.1");
    cache.store("nixpkgs", "def", "pkgs.firefox", "129.0");
    assert_eq!(
      cache.lookup("nixpkgs", "abc", "pkgs.firefox").as_deref(),
      Some("128.0")
    );
    assert_eq!(
      cache.lookup("nixpkgs", "def", "pkgs.firefox").as_deref(),
      Some("129.0")
    );
    assert_eq!(cache.lookup("nixpkgs", "abc", "pkgs.kate"), None);
    assert_eq!(cache.lookup("nixpkgs", "ghi", "pkgs.firefox"), None);
    assert_eq!(cache.lookup("other", "abc", "pkgs.firefox"), None);
  }

  #[test]
  fn save_then_load_preserves_entries() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("vc.json");
    let mut cache = VersionCache::default();
    cache.store("nixpkgs", "abc", "pkgs.firefox", "128.0");
    cache.store("nixpkgs-latest", "xyz", "pkgs.kate", "25.04");
    cache.save(&path).unwrap();

    let loaded = VersionCache::load(&path).unwrap();
    assert_eq!(loaded.entry_count(), 2);
    assert_eq!(
      loaded.lookup("nixpkgs", "abc", "pkgs.firefox").as_deref(),
      Some("128.0")
    );
    assert_eq!(
      loaded
        .lookup("nixpkgs-latest", "xyz", "pkgs.kate")
        .as_deref(),
      Some("25.04")
    );
  }

  #[test]
  fn save_creates_parent_directory() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("nested/parents/vc.json");
    let cache = VersionCache::default();
    cache.save(&path).unwrap();
    assert!(path.exists());
  }

  #[test]
  fn save_uses_0600_permissions_unix() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("vc.json");
    let cache = VersionCache::default();
    cache.save(&path).unwrap();
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      let mode =
        fs::metadata(&path).unwrap().permissions().mode() & 0o777;
      assert_eq!(mode, 0o600);
    }
  }

  #[test]
  fn store_overwrites_existing_value() {
    let mut cache = VersionCache::default();
    cache.store("nixpkgs", "abc", "pkgs.firefox", "128.0");
    cache.store("nixpkgs", "abc", "pkgs.firefox", "128.0.1");
    assert_eq!(
      cache.lookup("nixpkgs", "abc", "pkgs.firefox").as_deref(),
      Some("128.0.1")
    );
  }

  #[test]
  fn entry_count_aggregates_all_levels() {
    let mut cache = VersionCache::default();
    cache.store("a", "1", "x", "v");
    cache.store("a", "1", "y", "v");
    cache.store("a", "2", "x", "v");
    cache.store("b", "1", "x", "v");
    assert_eq!(cache.entry_count(), 4);
  }
}
