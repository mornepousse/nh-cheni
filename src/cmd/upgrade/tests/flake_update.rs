use super::*;

#[test]
fn parses_single_flake_input_update() {
    let stderr = "\
warning: Git tree '/home/user/nixos-config' is dirty
warning: updating lock file \"/home/user/nixos-config/flake.lock\":
• Updated input 'cheni':
    'gitlab:harrael/cheni/abc123?narHash=sha256-XXX=' (2026-04-19)
  → 'gitlab:harrael/cheni/def456?narHash=sha256-YYY=' (2026-04-20)
";
    let updates = parse_flake_update_events(stderr);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].name, "cheni");
    assert_eq!(updates[0].old_date, "2026-04-19");
    assert_eq!(updates[0].new_date, "2026-04-20");
}

#[test]
fn parses_multiple_flake_input_updates() {
    let stderr = "\
• Updated input 'nixpkgs':
    'github:NixOS/nixpkgs/aaa?narHash=sha256-A=' (2026-04-10)
  → 'github:NixOS/nixpkgs/bbb?narHash=sha256-B=' (2026-04-20)
• Updated input 'home-manager':
    'github:nix-community/home-manager/ccc?narHash=sha256-C=' (2026-04-15)
  → 'github:nix-community/home-manager/ddd?narHash=sha256-D=' (2026-04-20)
";
    let updates = parse_flake_update_events(stderr);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].name, "nixpkgs");
    assert_eq!(updates[1].name, "home-manager");
}

#[test]
fn parses_no_updates_on_a_clean_run() {
    let stderr = "warning: Git tree is dirty\nno updates\n";
    assert!(parse_flake_update_events(stderr).is_empty());
}

#[test]
fn parses_skips_malformed_date_lines() {
    // If the new-line locator is garbage, the event is skipped rather
    // than producing a malformed `InputUpdate`.
    let stderr = "\
• Updated input 'weird':
    'url' (not-a-date)
  → 'url' (2026-04-20)
";
    assert!(parse_flake_update_events(stderr).is_empty());
}

#[test]
fn detects_dirty_tree_warning_from_nix_stderr() {
    let stderr = "warning: Git tree '/home/mae/nixos-config' is dirty\n";
    assert!(detect_dirty_tree_warning(stderr));
}

#[test]
fn detects_dirty_tree_warning_older_nix_phrasing() {
    // Older nix wrote it as "dirty Git tree '…'" — just in case
    // the user pins a stale nix version somewhere.
    let stderr = "warning: dirty Git tree '/home/mae/nixos-config'\n";
    assert!(detect_dirty_tree_warning(stderr));
}

#[test]
fn detects_dirty_tree_warning_absent() {
    let stderr = "no updates\n";
    assert!(!detect_dirty_tree_warning(stderr));
}

// --- get_input_timestamp ---

/// Build a minimal flake.lock JSON value with two inputs
/// (`nixpkgs` at `base_ts` and `nixpkgs-latest` at `latest_ts`).
/// Passing `None` for an input omits it entirely so tests can
/// exercise the "absent input" branch.
fn make_lock(
    nixpkgs_ts: Option<u64>,
    latest_ts: Option<u64>,
) -> serde_json::Value {
    let mut nodes = serde_json::json!({
        "root": { "inputs": {} }
    });
    let mut root_inputs = serde_json::Map::new();
    if let Some(ts) = nixpkgs_ts {
        nodes["nixpkgs"] = serde_json::json!({
            "locked": { "lastModified": ts }
        });
        root_inputs.insert("nixpkgs".to_string(), serde_json::json!("nixpkgs"));
    }
    if let Some(ts) = latest_ts {
        nodes["nixpkgs-latest"] = serde_json::json!({
            "locked": { "lastModified": ts }
        });
        root_inputs.insert("nixpkgs-latest".to_string(), serde_json::json!("nixpkgs-latest"));
    }
    nodes["root"]["inputs"] = serde_json::Value::Object(root_inputs);
    serde_json::json!({ "nodes": nodes })
}

#[test]
fn get_input_timestamp_returns_last_modified_for_known_input() {
    let lock = make_lock(Some(1_700_000_000), Some(1_700_010_000));
    assert_eq!(get_input_timestamp(&lock, "nixpkgs"), Some(1_700_000_000));
    assert_eq!(get_input_timestamp(&lock, "nixpkgs-latest"), Some(1_700_010_000));
}

#[test]
fn get_input_timestamp_returns_none_when_input_absent() {
    let lock = make_lock(Some(1_700_000_000), None);
    assert_eq!(get_input_timestamp(&lock, "nixpkgs-latest"), None);
}

#[test]
fn get_input_timestamp_returns_none_on_malformed_lock() {
    let lock = serde_json::json!({ "not_nodes": {} });
    assert_eq!(get_input_timestamp(&lock, "nixpkgs"), None);
}

// --- check_nixpkgs_order (reads a real flake.lock file) ---

fn write_lock_file(dir: &std::path::Path, nixpkgs_ts: u64, latest_ts: u64) {
    let lock = serde_json::json!({
        "nodes": {
            "root": {
                "inputs": {
                    "nixpkgs": "nixpkgs",
                    "nixpkgs-latest": "nixpkgs-latest"
                }
            },
            "nixpkgs": {
                "locked": { "lastModified": nixpkgs_ts }
            },
            "nixpkgs-latest": {
                "locked": { "lastModified": latest_ts }
            }
        },
        "root": "root",
        "version": 7
    });
    std::fs::write(dir.join("flake.lock"), lock.to_string()).unwrap();
}

#[test]
fn check_nixpkgs_order_latest_is_newer() {
    let dir = tempfile::tempdir().unwrap();
    write_lock_file(dir.path(), 1_000_000, 2_000_000);
    assert_eq!(check_nixpkgs_order(dir.path()), InputOrder::LatestIsNewer);
}

#[test]
fn check_nixpkgs_order_same() {
    let dir = tempfile::tempdir().unwrap();
    write_lock_file(dir.path(), 1_500_000, 1_500_000);
    assert_eq!(check_nixpkgs_order(dir.path()), InputOrder::Same);
}

#[test]
fn check_nixpkgs_order_latest_is_older() {
    let dir = tempfile::tempdir().unwrap();
    write_lock_file(dir.path(), 2_000_000, 1_000_000);
    assert_eq!(check_nixpkgs_order(dir.path()), InputOrder::LatestIsOlder);
}

#[test]
fn check_nixpkgs_order_unknown_when_input_absent() {
    let dir = tempfile::tempdir().unwrap();
    // Only nixpkgs, no nixpkgs-latest.
    let lock = serde_json::json!({
        "nodes": {
            "root": { "inputs": { "nixpkgs": "nixpkgs" } },
            "nixpkgs": { "locked": { "lastModified": 1_000_000u64 } }
        },
        "root": "root",
        "version": 7
    });
    std::fs::write(dir.path().join("flake.lock"), lock.to_string()).unwrap();
    assert_eq!(check_nixpkgs_order(dir.path()), InputOrder::Unknown);
}

#[test]
fn check_nixpkgs_order_unknown_on_malformed_json() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("flake.lock"), "this is not json").unwrap();
    assert_eq!(check_nixpkgs_order(dir.path()), InputOrder::Unknown);
}

#[test]
fn check_nixpkgs_order_unknown_when_no_lock_file() {
    let dir = tempfile::tempdir().unwrap();
    // No flake.lock written → read_to_string fails.
    assert_eq!(check_nixpkgs_order(dir.path()), InputOrder::Unknown);
}
