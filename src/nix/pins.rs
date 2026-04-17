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
        .context("Failed to read package-pins.json")?;

    let pins: Vec<String> = serde_json::from_str(&content)
        .context("Failed to parse package-pins.json")?;

    debug!("Loaded {} pins", pins.len());
    Ok(pins)
}

/// Write the list of pinned packages.
pub fn write(config_dir: &Path, pins: &[String]) -> Result<()> {
    let path = config_dir.join("package-pins.json");

    let content = serde_json::to_string_pretty(pins)
        .context("Failed to serialize pins")?;

    std::fs::write(&path, format!("{}\n", content))
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
mod tests {
    use super::*;

    fn setup_temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn read_nonexistent_returns_empty() {
        let dir = setup_temp_dir();
        let pins = read(dir.path()).unwrap();
        assert!(pins.is_empty());
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = setup_temp_dir();
        let pins = vec!["legcord".to_string(), "vivaldi".to_string()];
        write(dir.path(), &pins).unwrap();
        let loaded = read(dir.path()).unwrap();
        assert_eq!(loaded, pins);
    }

    #[test]
    fn add_deduplicates() {
        let dir = setup_temp_dir();
        write(dir.path(), &["legcord".to_string()]).unwrap();

        let added = add(dir.path(), &["legcord".into(), "vivaldi".into()]).unwrap();
        assert_eq!(added, vec!["vivaldi".to_string()]);

        let pins = read(dir.path()).unwrap();
        assert_eq!(pins, vec!["legcord", "vivaldi"]);
    }

    #[test]
    fn remove_existing() {
        let dir = setup_temp_dir();
        write(dir.path(), &["a".into(), "b".into(), "c".into()]).unwrap();

        let removed = remove(dir.path(), &["b".into()]).unwrap();
        assert_eq!(removed, vec!["b".to_string()]);

        let pins = read(dir.path()).unwrap();
        assert_eq!(pins, vec!["a", "c"]);
    }

    #[test]
    fn remove_nonexistent_is_ok() {
        let dir = setup_temp_dir();
        write(dir.path(), &["a".into()]).unwrap();

        let removed = remove(dir.path(), &["z".into()]).unwrap();
        assert!(removed.is_empty());
    }

    #[test]
    fn clear_all() {
        let dir = setup_temp_dir();
        write(dir.path(), &["a".into(), "b".into(), "c".into()]).unwrap();

        let count = clear(dir.path()).unwrap();
        assert_eq!(count, 3);

        let pins = read(dir.path()).unwrap();
        assert!(pins.is_empty());
    }
}
