//! Resolve the cheni cache directory and file paths, and create it
//! privately.
//!
//! `$XDG_CACHE_HOME/cheni/`, falling back to `$HOME/.cache/cheni/`, then
//! `/tmp/cheni/`. This (and the private-dir creation) was duplicated
//! bit-for-bit in `timeline.rs` and `version_cache.rs`; lifted here when
//! `error_corpus.rs` became the third caller (the `cheni_util`
//! convention: extract on the third copy).

use std::{
  fs,
  path::{Path, PathBuf},
};

/// The cheni cache directory. Not created here — call [`ensure_dir`]
/// before writing into it.
#[must_use]
pub fn dir() -> PathBuf {
  if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
    return PathBuf::from(xdg).join("cheni");
  }
  if let Some(home) = std::env::var_os("HOME") {
    return PathBuf::from(home).join(".cache").join("cheni");
  }
  PathBuf::from("/tmp").join("cheni")
}

/// Path to `name` inside the cheni cache directory.
#[must_use]
pub fn file(name: &str) -> PathBuf {
  dir().join(name)
}

/// Create `dir` (typically a cache-file's parent) with mode 0o700 on
/// Unix so the file listing stays private to the user. Plain
/// `create_dir_all` uses the process umask (typically 0o022 → 0o755,
/// world-readable listing), which would leak the existence of cached
/// entries. Idempotent (recursive).
///
/// # Errors
///
/// Returns the underlying IO error if the directory can't be created.
pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
  #[cfg(unix)]
  {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
  }
  #[cfg(not(unix))]
  {
    fs::create_dir_all(dir)
  }
}
