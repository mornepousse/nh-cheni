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
