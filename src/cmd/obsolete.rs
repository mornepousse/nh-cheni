//! Shared helper for detecting obsolete pins.
//!
//! A pin is "obsolete" when nixpkgs has caught up with (or passed)
//! nixpkgs-latest, which happens after a regular `upgrade`.
//! In that case, the pins have no effect and can be removed.

use std::path::Path;

use tracing::debug;

/// Count the number of obsolete pins.
///
/// Returns the pin count if nixpkgs >= nixpkgs-latest (all obsolete),
/// otherwise returns 0 (pins are still useful).
pub fn count_obsolete_pins(lock_path: &Path, current_pins: &[String]) -> usize {
    if current_pins.is_empty() {
        return 0;
    }

    let is_obsolete = are_pins_obsolete(lock_path);
    if is_obsolete {
        current_pins.len()
    } else {
        0
    }
}

/// Check whether nixpkgs has caught up with nixpkgs-latest.
///
/// Compares the `lastModified` timestamps in flake.lock.
/// Returns true if nixpkgs >= nixpkgs-latest (pins are obsolete).
fn are_pins_obsolete(lock_path: &Path) -> bool {
    let content = match std::fs::read_to_string(lock_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let lock: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let base_time = get_input_timestamp(&lock, "nixpkgs");
    let latest_time = get_input_timestamp(&lock, "nixpkgs-latest");

    match (base_time, latest_time) {
        (Some(base), Some(latest)) => {
            debug!("nixpkgs: {}, nixpkgs-latest: {}", base, latest);
            // Pins are obsolete when nixpkgs is at the same level or ahead
            base >= latest
        }
        _ => false,
    }
}

/// Extract the lastModified timestamp for a flake input (resolves via root).
/// Handles indirection: root.inputs[name] may point to "nixpkgs_4".
fn get_input_timestamp(lock: &serde_json::Value, name: &str) -> Option<u64> {
    // Resolve root input to the actual node name
    let root_input = lock.get("nodes")
        .and_then(|n| n.get("root"))
        .and_then(|r| r.get("inputs"))
        .and_then(|i| i.get(name));

    let node_name = root_input
        .and_then(|v| v.as_str())
        .unwrap_or(name);

    lock.get("nodes")?
        .get(node_name)?
        .get("locked")?
        .get("lastModified")?
        .as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a fake flake.lock with the given timestamps.
    /// Includes root.inputs mapping to simulate real flake.lock structure.
    fn write_fake_lock(dir: &Path, nixpkgs_ts: u64, latest_ts: u64) {
        let lock_content = serde_json::json!({
            "nodes": {
                "nixpkgs": {
                    "locked": {
                        "lastModified": nixpkgs_ts,
                        "rev": "abc123"
                    }
                },
                "nixpkgs-latest": {
                    "locked": {
                        "lastModified": latest_ts,
                        "rev": "def456"
                    }
                },
                "root": {
                    "inputs": {
                        "nixpkgs": "nixpkgs",
                        "nixpkgs-latest": "nixpkgs-latest"
                    }
                }
            },
            "root": "root",
            "version": 7
        });

        let path = dir.join("flake.lock");
        std::fs::write(&path, serde_json::to_string_pretty(&lock_content).unwrap())
            .unwrap();
    }

    #[test]
    fn no_pins_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_lock(dir.path(), 100, 100);

        let lock_path = dir.path().join("flake.lock");
        let count = count_obsolete_pins(&lock_path, &[]);
        assert_eq!(count, 0);
    }

    #[test]
    fn nixpkgs_caught_up_returns_all_pins() {
        let dir = tempfile::tempdir().unwrap();
        // nixpkgs (200) >= nixpkgs-latest (100) -> obsolete
        write_fake_lock(dir.path(), 200, 100);

        let lock_path = dir.path().join("flake.lock");
        let pins = vec!["firefox".to_string(), "legcord".to_string()];
        let count = count_obsolete_pins(&lock_path, &pins);
        assert_eq!(count, 2);
    }

    #[test]
    fn nixpkgs_same_as_latest_returns_all_pins() {
        let dir = tempfile::tempdir().unwrap();
        // nixpkgs == nixpkgs-latest -> obsolete
        write_fake_lock(dir.path(), 100, 100);

        let lock_path = dir.path().join("flake.lock");
        let pins = vec!["vivaldi".to_string()];
        let count = count_obsolete_pins(&lock_path, &pins);
        assert_eq!(count, 1);
    }

    #[test]
    fn nixpkgs_behind_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        // nixpkgs (100) < nixpkgs-latest (200) -> pins still active
        write_fake_lock(dir.path(), 100, 200);

        let lock_path = dir.path().join("flake.lock");
        let pins = vec!["firefox".to_string()];
        let count = count_obsolete_pins(&lock_path, &pins);
        assert_eq!(count, 0);
    }

    #[test]
    fn missing_lock_file_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        // No flake.lock -> no info, don't touch the pins
        let lock_path = dir.path().join("flake.lock");
        let pins = vec!["firefox".to_string()];
        let count = count_obsolete_pins(&lock_path, &pins);
        assert_eq!(count, 0);
    }
}
