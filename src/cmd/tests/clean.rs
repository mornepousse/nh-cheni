//! Tests for `cmd::clean`.

#![allow(unused_imports)]

use super::*;
use std::collections::HashSet;

fn declared(names: &[&str]) -> HashSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn find_orphan_pins_returns_pins_not_in_declared() {
    let pins = vec!["firefox".to_string(), "kicad".to_string()];
    let decl = declared(&["kicad"]);
    let orphans = find_orphan_pins(&pins, &decl);
    assert_eq!(orphans, vec!["firefox".to_string()]);
}

#[test]
fn find_orphan_pins_handles_empty_pins() {
    let decl = declared(&["firefox"]);
    let orphans = find_orphan_pins(&[], &decl);
    assert!(orphans.is_empty());
}

#[test]
fn find_orphan_pins_handles_all_declared() {
    let pins = vec!["firefox".to_string(), "kicad".to_string()];
    let decl = declared(&["firefox", "kicad", "vivaldi"]);
    let orphans = find_orphan_pins(&pins, &decl);
    assert!(orphans.is_empty());
}

#[test]
fn find_orphan_pins_when_no_modules_returns_all_as_orphans() {
    let pins = vec!["firefox".to_string()];
    let decl = HashSet::new();
    let orphans = find_orphan_pins(&pins, &decl);
    assert_eq!(orphans, vec!["firefox".to_string()]);
}

#[test]
fn find_result_symlinks_in_tempdir() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let target = dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("result")).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("result-1")).unwrap();
    std::fs::write(dir.path().join("flake.nix"), "").unwrap();

    let mut found = find_result_symlinks(dir.path());
    found.sort();
    let names: Vec<String> = found
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["result".to_string(), "result-1".to_string()]);
}

#[test]
fn find_result_symlinks_ignores_non_results() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let target = dir.path().join("t");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("flake.nix")).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("hello")).unwrap();

    let found = find_result_symlinks(dir.path());
    assert!(found.is_empty());
}

#[test]
fn find_orphan_freezes_returns_freezes_not_in_declared() {
    let mut freezes = std::collections::BTreeMap::new();
    freezes.insert(
        "firefox".to_string(),
        crate::nix::freezes::FreezeEntry {
            rev: "abc123".into(),
            nar_hash: "sha256-abc".into(),
            version: "1.0".into(),
            frozen_at: "2026-04-28".into(),
            major_constraint: None,
        },
    );
    freezes.insert(
        "kicad".to_string(),
        crate::nix::freezes::FreezeEntry {
            rev: "def456".into(),
            nar_hash: "sha256-def".into(),
            version: "2.0".into(),
            frozen_at: "2026-04-28".into(),
            major_constraint: None,
        },
    );
    let decl = declared(&["kicad"]);
    let mut orphans = find_orphan_freezes(&freezes, &decl);
    orphans.sort();
    assert_eq!(orphans, vec!["firefox".to_string()]);
}
