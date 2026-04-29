//! Tests for `cmd::snapshot`.

#![allow(unused_imports)]

use std::collections::BTreeMap;

use super::*;

fn make_freeze(version: &str) -> freezes::FreezeEntry {
    freezes::FreezeEntry {
        rev: "abc".into(),
        nar_hash: "sha256-x".into(),
        version: version.into(),
        frozen_at: "2026-04-28".into(),
        major_constraint: None,
    }
}

fn make_freeze_with_rev(version: &str, rev: &str) -> freezes::FreezeEntry {
    freezes::FreezeEntry {
        rev: rev.into(),
        nar_hash: "sha256-x".into(),
        version: version.into(),
        frozen_at: "2026-04-28".into(),
        major_constraint: None,
    }
}

#[test]
fn compose_snapshot_includes_all_pins_and_freezes() {
    let pins = vec!["firefox".into(), "kicad".into()];
    let mut freezes = BTreeMap::new();
    freezes.insert("vivaldi".to_string(), make_freeze("7.0"));
    let snap = compose_snapshot(pins.clone(), freezes.clone(), "host1");
    assert_eq!(snap.format_version, FORMAT_VERSION);
    assert_eq!(snap.pins, pins);
    assert_eq!(snap.freezes.len(), 1);
    assert_eq!(snap.hostname, "host1");
}

#[test]
fn compute_diff_empty_when_identical() {
    let pins = vec!["firefox".to_string()];
    let mut freezes = BTreeMap::new();
    freezes.insert("vivaldi".to_string(), make_freeze("7.0"));
    let snap = compose_snapshot(pins.clone(), freezes.clone(), "host1");
    let diff = compute_diff(&pins, &freezes, &snap);
    assert!(diff.is_empty());
}

#[test]
fn compute_diff_detects_added_and_removed_pins() {
    let current_pins = vec!["firefox".to_string(), "kicad".to_string()];
    let current_freezes = BTreeMap::new();
    let snap = compose_snapshot(
        vec!["firefox".to_string(), "vivaldi".to_string()],
        BTreeMap::new(),
        "host1",
    );
    let diff = compute_diff(&current_pins, &current_freezes, &snap);
    assert_eq!(diff.pins_added, vec!["vivaldi".to_string()]);
    assert_eq!(diff.pins_removed, vec!["kicad".to_string()]);
}

#[test]
fn compute_diff_detects_changed_freeze() {
    let current_pins: Vec<String> = vec![];
    let mut current_freezes = BTreeMap::new();
    current_freezes.insert("vivaldi".to_string(), make_freeze_with_rev("7.0", "old"));
    let mut snap_freezes = BTreeMap::new();
    snap_freezes.insert("vivaldi".to_string(), make_freeze_with_rev("7.1", "new"));
    let snap = compose_snapshot(vec![], snap_freezes, "host1");
    let diff = compute_diff(&current_pins, &current_freezes, &snap);
    assert_eq!(diff.freezes_changed, vec!["vivaldi".to_string()]);
    assert!(diff.freezes_added.is_empty());
    assert!(diff.freezes_removed.is_empty());
}

#[test]
fn compute_diff_detects_added_and_removed_freezes() {
    let current_pins: Vec<String> = vec![];
    let mut current_freezes = BTreeMap::new();
    current_freezes.insert("kicad".to_string(), make_freeze("10.0"));
    let mut snap_freezes = BTreeMap::new();
    snap_freezes.insert("vivaldi".to_string(), make_freeze("7.0"));
    let snap = compose_snapshot(vec![], snap_freezes, "host1");
    let diff = compute_diff(&current_pins, &current_freezes, &snap);
    assert_eq!(diff.freezes_added, vec!["vivaldi".to_string()]);
    assert_eq!(diff.freezes_removed, vec!["kicad".to_string()]);
}
