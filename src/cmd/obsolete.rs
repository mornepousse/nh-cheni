//! Shared helper for detecting obsolete pins.
//!
//! Un pin est « obsolète » quand nixpkgs a rattrapé (ou dépassé)
//! nixpkgs-latest, ce qui arrive après un `upgrade` classique.
//! Dans ce cas, les pins n'ont plus d'effet et peuvent être supprimés.

use std::path::Path;

use tracing::debug;

/// Compte le nombre de pins obsolètes.
///
/// Retourne le nombre de pins si nixpkgs >= nixpkgs-latest (tous obsolètes),
/// sinon retourne 0 (les pins sont encore utiles).
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

/// Vérifie si nixpkgs a rattrapé nixpkgs-latest.
///
/// Compare les timestamps `lastModified` dans flake.lock.
/// Retourne true si nixpkgs >= nixpkgs-latest (pins obsolètes).
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
            // Les pins sont obsolètes si nixpkgs est au même niveau ou devant
            base >= latest
        }
        _ => false,
    }
}

/// Extrait le timestamp lastModified d'un input dans flake.lock.
fn get_input_timestamp(lock: &serde_json::Value, name: &str) -> Option<u64> {
    lock.get("nodes")?
        .get(name)?
        .get("locked")?
        .get("lastModified")?
        .as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Crée un flake.lock factice avec les timestamps donnés.
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
                "root": {}
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
        // nixpkgs (200) >= nixpkgs-latest (100) → obsolète
        write_fake_lock(dir.path(), 200, 100);

        let lock_path = dir.path().join("flake.lock");
        let pins = vec!["firefox".to_string(), "legcord".to_string()];
        let count = count_obsolete_pins(&lock_path, &pins);
        assert_eq!(count, 2);
    }

    #[test]
    fn nixpkgs_same_as_latest_returns_all_pins() {
        let dir = tempfile::tempdir().unwrap();
        // nixpkgs == nixpkgs-latest → obsolète
        write_fake_lock(dir.path(), 100, 100);

        let lock_path = dir.path().join("flake.lock");
        let pins = vec!["vivaldi".to_string()];
        let count = count_obsolete_pins(&lock_path, &pins);
        assert_eq!(count, 1);
    }

    #[test]
    fn nixpkgs_behind_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        // nixpkgs (100) < nixpkgs-latest (200) → pins encore actifs
        write_fake_lock(dir.path(), 100, 200);

        let lock_path = dir.path().join("flake.lock");
        let pins = vec!["firefox".to_string()];
        let count = count_obsolete_pins(&lock_path, &pins);
        assert_eq!(count, 0);
    }

    #[test]
    fn missing_lock_file_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        // Pas de flake.lock → pas d'info, on ne touche pas aux pins
        let lock_path = dir.path().join("flake.lock");
        let pins = vec!["firefox".to_string()];
        let count = count_obsolete_pins(&lock_path, &pins);
        assert_eq!(count, 0);
    }
}
