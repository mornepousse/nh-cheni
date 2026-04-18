//! Package pins management.
//!
//! Reads and writes `package-pins.json` in the NixOS config directory.
//! Pinned packages are pulled from `nixpkgs-latest` via an overlay
//! instead of the regular `nixpkgs`.

use std::path::Path;

use anyhow::{Context, Result};
use tracing::debug;

/// Read the current list of pinned packages.
pub fn read(config_dir: &Path) -> Result<Vec<String>> {
    let path = config_dir.join("package-pins.json");

    if !path.exists() {
        debug!("No package-pins.json found");
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    // Empty file is a valid "no pins" state — friendlier than failing
    // to parse `""` as JSON. Users running `cheni unpin --all` followed
    // by an editor that empties the file would otherwise get a cryptic
    // serde error.
    if content.trim().is_empty() {
        debug!("package-pins.json is empty, treating as no pins");
        return Ok(Vec::new());
    }

    let pins: Vec<String> = serde_json::from_str(&content).with_context(|| {
        format!(
            "package-pins.json is not valid JSON.\n  \
             Path: {}\n  \
             Expected: a JSON array of package names, e.g. [\"vivaldi\", \"mesa\"]\n  \
             Fix: edit the file, or reset with: echo '[]' > {}",
            path.display(),
            path.display()
        )
    })?;

    debug!("Loaded {} pins", pins.len());
    Ok(pins)
}

/// Write the list of pinned packages.
pub fn write(config_dir: &Path, pins: &[String]) -> Result<()> {
    let path = config_dir.join("package-pins.json");

    let content = serde_json::to_string_pretty(pins)
        .context("Failed to serialize pins")?;

    // Atomic write: package-pins.json is read by the Nix overlay at
    // every eval, so a truncated / half-JSON file would break
    // 'nixos-rebuild' system-wide. The tmp-file-then-rename pattern
    // guarantees readers see the old or new content, never a mix.
    crate::util::atomic_write(&path, &format!("{}\n", content))
        .context("Failed to write package-pins.json")?;

    debug!("Wrote {} pins to package-pins.json", pins.len());
    Ok(())
}

/// Add packages to the pin list.
///
/// Returns the names that were actually added (excludes duplicates).
pub fn add(config_dir: &Path, names: &[String]) -> Result<Vec<String>> {
    let mut pins = read(config_dir)?;
    let mut added = Vec::new();

    for name in names {
        if !pins.contains(name) {
            pins.push(name.clone());
            added.push(name.clone());
        } else {
            debug!("{} already pinned", name);
        }
    }

    pins.sort();
    write(config_dir, &pins)?;
    Ok(added)
}

/// Remove packages from the pin list.
///
/// Returns the names that were actually removed.
pub fn remove(config_dir: &Path, names: &[String]) -> Result<Vec<String>> {
    let mut pins = read(config_dir)?;
    let mut removed = Vec::new();

    for name in names {
        if pins.contains(name) {
            pins.retain(|p| p != name);
            removed.push(name.clone());
        } else {
            debug!("{} was not pinned", name);
        }
    }

    write(config_dir, &pins)?;
    Ok(removed)
}

/// Remove all pins.
pub fn clear(config_dir: &Path) -> Result<usize> {
    let pins = read(config_dir)?;
    let count = pins.len();
    write(config_dir, &[])?;
    Ok(count)
}

#[cfg(test)]
#[path = "tests/pins.rs"]
mod tests;
