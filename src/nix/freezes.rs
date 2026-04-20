//! Package freezes management.
//!
//! Reads and writes `package-freezes.json` in the NixOS config directory.
//! A frozen package is held at a specific nixpkgs revision while the rest
//! of the system continues to update — the **opposite** semantic of
//! `package-pins.json`, which routes through `nixpkgs-latest` to get a
//! *newer* version.
//!
//! File layout (stable, map keyed by package name):
//!
//! ```json
//! {
//!   "firefox": {
//!     "rev": "abc123…",
//!     "narHash": "sha256-…",
//!     "version": "127.0.1",
//!     "frozen_at": "2026-04-20"
//!   }
//! }
//! ```
//!
//! Only `rev` + `narHash` are load-bearing — the overlay uses them to
//! call `builtins.fetchTree`. `version` and `frozen_at` are diagnostic,
//! shown in `cheni status` / `cheni freeze` (list) so the user knows
//! what they held and when.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// A frozen package entry. Serialised exactly as documented at the
/// module level — keep the field names stable; the overlay in flake.nix
/// reads `rev` and `narHash` by name.
///
/// `rev` and `narHash` are load-bearing (the overlay fails hard if
/// either is missing). `version` and `frozen_at` are diagnostic-only
/// and default to empty when absent — that way a freezes file written
/// by an older cheni still loads cleanly after a schema evolution.
///
/// `major_constraint` is the "track this major, block everything else"
/// knob (set via `cheni freeze <pkg> --major N`). When `None`, the
/// freeze is a strict lock: `rev`/`narHash` never bump until the user
/// unfreezes. When `Some(N)`, `cheni upgrade` runs a refresh pass
/// that bumps `rev`/`narHash` to today's nixpkgs so long as upstream
/// is still on major N; once upstream moves to N+1, the entry is
/// held and the user gets a visible notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreezeEntry {
    /// Full nixpkgs git revision the package is held at.
    pub rev: String,
    /// `nix flake prefetch` narHash for the corresponding tarball —
    /// `sha256-…` form. Lets the overlay's `fetchTree` be pure.
    #[serde(rename = "narHash")]
    pub nar_hash: String,
    /// Installed version at freeze time (diagnostic — shown in status).
    #[serde(default)]
    pub version: String,
    /// ISO `YYYY-MM-DD` date when the freeze was created (diagnostic).
    #[serde(default)]
    pub frozen_at: String,
    /// Major-version constraint. `None` = strict lock (never auto-bump).
    /// `Some(N)` = accept any `N.y.z`, block `(N+1).y.z` and above.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "majorConstraint")]
    pub major_constraint: Option<u32>,
}

/// Map form used across the module — `BTreeMap` so the JSON on disk has
/// a deterministic key order (diff-friendly across cheni runs).
pub type Freezes = BTreeMap<String, FreezeEntry>;

/// Read the current freeze map.
///
/// Returns an empty map when the file doesn't exist, is empty, or is
/// whitespace-only — same friendly-degradation contract as `pins::read`.
pub fn read(config_dir: &Path) -> Result<Freezes> {
    let path = config_dir.join("package-freezes.json");

    if !path.exists() {
        debug!("No package-freezes.json found");
        return Ok(BTreeMap::new());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    // Empty file = no freezes. Same rationale as pins::read — editors
    // that empty the file on save shouldn't break cheni.
    if content.trim().is_empty() {
        debug!("package-freezes.json is empty, treating as no freezes");
        return Ok(BTreeMap::new());
    }

    let freezes: Freezes = serde_json::from_str(&content).with_context(|| {
        format!(
            "package-freezes.json is not valid JSON.\n  \
             Path: {}\n  \
             Expected: a JSON object of {{ name: {{rev, narHash, version, frozen_at}} }}\n  \
             Fix: edit the file, or reset with: echo '{{}}' > {}",
            path.display(),
            path.display()
        )
    })?;

    debug!("Loaded {} freezes", freezes.len());
    Ok(freezes)
}

/// Write the full freeze map atomically.
///
/// Atomic write: `package-freezes.json` is read by the Nix overlay at
/// every eval — a truncated or half-JSON file would break `nixos-rebuild`
/// system-wide, identical to the reasoning in `pins::write`.
pub fn write(config_dir: &Path, freezes: &Freezes) -> Result<()> {
    let path = config_dir.join("package-freezes.json");

    let content = serde_json::to_string_pretty(freezes)
        .context("Failed to serialize freezes")?;

    crate::util::atomic_write(&path, &format!("{}\n", content))
        .context("Failed to write package-freezes.json")?;

    debug!("Wrote {} freezes to package-freezes.json", freezes.len());
    Ok(())
}

/// Add a single freeze, replacing any existing entry for the same name.
///
/// Returns `true` when the name was newly frozen, `false` when we replaced
/// an existing entry (caller typically wants to log the difference).
///
/// Validates the package name before storing — same ruleset as
/// `pins::add` since the name flows into the overlay attribute lookup.
pub fn add(config_dir: &Path, name: &str, entry: FreezeEntry) -> Result<bool> {
    validate_package_name(name)?;
    validate_entry(&entry)?;

    let mut freezes = read(config_dir)?;
    let inserted_new = !freezes.contains_key(name);
    freezes.insert(name.to_string(), entry);
    write(config_dir, &freezes)?;
    Ok(inserted_new)
}

/// Remove freezes by name. Returns the names that were actually removed
/// (mirrors `pins::remove` so the two commands have the same shape).
pub fn remove(config_dir: &Path, names: &[String]) -> Result<Vec<String>> {
    let mut freezes = read(config_dir)?;
    let mut removed = Vec::new();
    for name in names {
        if freezes.remove(name).is_some() {
            removed.push(name.clone());
        } else {
            debug!("{} was not frozen", name);
        }
    }
    write(config_dir, &freezes)?;
    Ok(removed)
}

/// Remove every freeze. Returns how many were removed.
pub fn clear(config_dir: &Path) -> Result<usize> {
    let freezes = read(config_dir)?;
    let count = freezes.len();
    write(config_dir, &BTreeMap::new())?;
    Ok(count)
}

/// Reject obviously bogus package names before storing them.
///
/// Same ruleset as `pins::validate_package_name` — ASCII letters, digits
/// and a few nixpkgs separators. Names with control chars, slashes or
/// quotes would break the Nix attribute lookup (`pkgs-at-rev.${name}`)
/// or pollute logs.
fn validate_package_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Package name is empty");
    }
    if name.len() > 128 {
        anyhow::bail!(
            "Package name '{}…' is suspiciously long ({} chars, max 128)",
            &name.chars().take(20).collect::<String>(),
            name.len()
        );
    }
    if let Some(bad) = name.chars().find(|c| {
        c.is_control()
            || *c == '\n'
            || *c == '\r'
            || *c == '/'
            || *c == '\\'
            || *c == '"'
            || *c == '\''
    }) {
        anyhow::bail!(
            "Package name '{}' contains an invalid character ({:?}). \
             Nix package names use letters, digits, '-', '_', '.', '+'.",
            name,
            bad
        );
    }
    Ok(())
}

/// Reject a freeze entry with a malformed `rev` or `narHash`.
///
/// Both are splice-critical: `rev` goes into a `github:...` URL, and
/// `narHash` into `builtins.fetchTree`. We reject anything that could
/// embed a shell metachar, a newline or a quote, not because the overlay
/// shells out (it doesn't), but because a malformed value propagating to
/// a broken flake eval produces a much worse error surface than failing
/// at write time.
fn validate_entry(entry: &FreezeEntry) -> Result<()> {
    // Git rev: 40-char hex. We accept 7..=64 to leave room for short
    // hashes (diagnostic use) and alternative future hash types.
    if entry.rev.len() < 7 || entry.rev.len() > 64 {
        anyhow::bail!(
            "Freeze rev has an unusual length ({} chars, expected 7..=64): {:?}",
            entry.rev.len(),
            entry.rev
        );
    }
    if !entry.rev.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!(
            "Freeze rev is not a hex git hash: {:?}",
            entry.rev
        );
    }

    // narHash: `sha256-…` or `sha512-…` (base64, SRI form). We keep this
    // permissive on the body — just reject the obvious injection vectors.
    if !entry.nar_hash.starts_with("sha256-") && !entry.nar_hash.starts_with("sha512-") {
        anyhow::bail!(
            "Freeze narHash doesn't look like an SRI hash (expected sha256-… or sha512-…): {:?}",
            entry.nar_hash
        );
    }
    if entry.nar_hash.len() > 200 {
        anyhow::bail!(
            "Freeze narHash is suspiciously long ({} chars)",
            entry.nar_hash.len()
        );
    }
    if entry.nar_hash.chars().any(|c| c.is_control() || c == '"' || c == '\\') {
        anyhow::bail!(
            "Freeze narHash contains an invalid character: {:?}",
            entry.nar_hash
        );
    }

    // Version and frozen_at are diagnostic. Reject newlines / control
    // chars so they stay printable in `cheni status`, but otherwise leave
    // them alone (versions like `1.0-rc2+git20240101` are real).
    for (field, value) in [("version", &entry.version), ("frozen_at", &entry.frozen_at)] {
        if value.chars().any(|c| c.is_control()) {
            anyhow::bail!("Freeze {} contains a control character: {:?}", field, value);
        }
        if value.len() > 128 {
            anyhow::bail!("Freeze {} is suspiciously long ({} chars)", field, value.len());
        }
    }

    // Major constraint: no software has a major version past ~9999 in
    // practice (kernels, browsers, etc. all use 2-3 digits). A value
    // past this range is almost certainly a typo or a payload edit —
    // reject rather than let it round-trip quietly through the JSON.
    if let Some(n) = entry.major_constraint {
        if n > 9999 {
            anyhow::bail!(
                "Freeze majorConstraint is implausibly large ({}). \
                 Valid majors are in 0..=9999.",
                n
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/freezes.rs"]
mod tests;
